#![forbid(unsafe_code)]

use std::sync::Arc;

use audiodown_credential_vault::{load_or_create_master_key, CredentialVault};
use audiodown_plugin_manager::{github::GitHubClient, service::PluginManagerService};
use audiodown_server::{
    app::build_router,
    config::Config,
    lifecycle::run_lifecycle_reconciler,
    plugin_manager_adapters::{
        ConfiguredLifecycleRiskAuthorizer, SqlitePluginManagerStore, SupervisorPluginRuntime,
    },
    proxy_adapters::{SqliteCoreProxyBackend, SqliteVaultRepository},
    proxy_gateway::{CoreProxyBackend, ProxyGateway, ProxyTokenRegistry},
    shutdown::{
        finish_ordered_shutdown, wait_for_core_task_exit, CoreTaskExit, ShutdownOrder,
        ShutdownPhase,
    },
    state::{AppState, DevelopmentConfig, ProxyRuntimeState},
    supervisor::UnixSupervisorClient,
};
use audiodown_storage::Storage;
use tokio::{net::TcpListener, sync::watch, task::JoinSet};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    tokio::fs::create_dir_all(&config.data_dir).await?;
    let master_key = load_or_create_master_key(config.master_key_path())?;
    tokio::fs::create_dir_all(config.data_dir.join("logs")).await?;
    tokio::fs::create_dir_all(config.data_dir.join("plugins")).await?;

    let _logging_guard =
        audiodown_logging::init_logging(&config.data_dir.join("logs"), &config.log_filter)?;
    let storage = Storage::connect(&config.database_url).await?;
    storage.migrate().await?;

    let vault = CredentialVault::new(master_key, SqliteVaultRepository::new(storage.clone()));
    let proxy_backend = Arc::new(SqliteCoreProxyBackend::production(storage.clone(), vault));
    let proxy_tokens = Arc::new(ProxyTokenRegistry::new());
    let gateway_backend: Arc<dyn CoreProxyBackend> = proxy_backend.clone();
    let proxy_gateway = ProxyGateway::bind(
        &config.proxy_socket,
        Arc::clone(&proxy_tokens),
        gateway_backend,
    )
    .await?;
    let proxy_runtime = Arc::new(ProxyRuntimeState::new(
        Arc::clone(&proxy_tokens),
        proxy_backend,
    ));

    let supervisor = Arc::new(UnixSupervisorClient::new(
        &config.supervisor_socket,
        &config.core_token_file,
    ));
    let development = DevelopmentConfig {
        enabled: config.dev_mode,
        token: config.dev_token,
    };
    let repository_source = Arc::new(GitHubClient::new(
        &config.github_api_base,
        &config.github_archive_base,
    )?);
    let plugin_manager = Arc::new(
        PluginManagerService::new(
            Arc::new(SqlitePluginManagerStore::new(storage.clone())),
            repository_source,
            config.data_dir.join("plugins"),
            semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
            semver::Version::new(1, 0, 0),
        )
        .with_installation_ports(
            Arc::new(SupervisorPluginRuntime::with_proxy_tokens(
                supervisor.clone(),
                Arc::clone(&proxy_tokens),
            )),
            Arc::new(ConfiguredLifecycleRiskAuthorizer::new(development.clone())),
        ),
    );
    let startup_cleanup = plugin_manager.cleanup_all_runtimes().await?;
    if startup_cleanup.failed > 0 {
        anyhow::bail!(
            "Core startup runtime cleanup failed for {} of {} plugins",
            startup_cleanup.failed,
            startup_cleanup.scanned
        );
    }
    if let Err(error) = plugin_manager.reconcile_install_operations().await {
        tracing::warn!(error = %error, "Plugin install reconciliation will retry on next startup");
    }
    let lifecycle_manager = plugin_manager.clone();
    let state = AppState::new(
        storage,
        semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        supervisor,
    )
    .with_plugin_manager(plugin_manager.clone())
    .with_development(development.enabled, development.token)
    .with_proxy_runtime(proxy_runtime);
    let app = build_router(state);
    let listener = TcpListener::bind(config.bind).await?;
    tracing::info!(address = %config.bind, "AudioDown Core listening");

    let (work_shutdown_tx, work_shutdown_rx) = watch::channel(false);
    let (gateway_shutdown_tx, gateway_shutdown_rx) = watch::channel(false);
    let mut work_tasks = JoinSet::new();

    let mut gateway_task = tokio::spawn(async move {
        proxy_gateway
            .run(gateway_shutdown_rx)
            .await
            .map_err(anyhow::Error::from)
    });

    let lifecycle_shutdown = work_shutdown_rx.clone();
    work_tasks.spawn(async move {
        run_lifecycle_reconciler(
            lifecycle_manager,
            config.plugin_reconcile_interval,
            config.plugin_idle_timeout,
            lifecycle_shutdown,
        )
        .await;
        ("lifecycle reconciler", Ok::<(), anyhow::Error>(()))
    });

    let server_shutdown = work_shutdown_rx;
    work_tasks.spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(wait_for_shutdown(server_shutdown))
            .await
            .map_err(anyhow::Error::from);
        ("HTTP server", result)
    });

    let mut early_exit = None;
    let mut early_failure = None;
    let mut gateway_completed_early = false;
    tokio::select! {
        () = shutdown_signal() => {}
        completed = wait_for_core_task_exit(&mut work_tasks, &mut gateway_task) => {
            match completed {
                CoreTaskExit::Work { name, result } => early_exit = Some((name, result)),
                CoreTaskExit::Gateway { result } => {
                    gateway_completed_early = true;
                    early_exit = Some(("proxy gateway", result));
                }
                CoreTaskExit::WorkJoinFailed(error) => early_failure = Some(error),
                CoreTaskExit::GatewayJoinFailed(error) => {
                    gateway_completed_early = true;
                    early_failure = Some(error);
                }
                CoreTaskExit::WorkSetEmpty => {
                    early_failure = Some(anyhow::anyhow!("Core task set was empty"));
                }
            }
        }
    }
    let _ = work_shutdown_tx.send(true);

    let mut failure = early_failure;
    if let Some((name, result)) = early_exit {
        failure = Some(match result {
            Ok(()) => anyhow::anyhow!("{name} exited unexpectedly"),
            Err(error) => error.context(format!("{name} failed")),
        });
    }
    while let Some(completed) = work_tasks.join_next().await {
        match completed {
            Ok((name, result)) => {
                if let Err(error) = result {
                    failure.get_or_insert_with(|| error.context(format!("{name} failed")));
                }
            }
            Err(error) => {
                failure.get_or_insert_with(|| {
                    anyhow::Error::new(error).context("failed to join Core work task")
                });
            }
        }
    }

    let shutdown_order = ShutdownOrder::default();
    shutdown_order.record(ShutdownPhase::WorkQuiesced);
    let cleanup_manager = plugin_manager.clone();
    let cleanup = async move {
        let report = cleanup_manager
            .cleanup_all_runtimes()
            .await
            .map_err(anyhow::Error::from)?;
        if report.failed > 0 {
            tracing::error!(
                failed = report.failed,
                scanned = report.scanned,
                "Core runtime cleanup did not complete for every plugin"
            );
            anyhow::bail!(
                "Core runtime cleanup failed for {} of {} plugins",
                report.failed,
                report.scanned
            );
        }
        Ok::<(), anyhow::Error>(())
    };
    let gateway_shutdown = async move {
        if gateway_completed_early {
            return Ok::<(), anyhow::Error>(());
        }
        let _ = gateway_shutdown_tx.send(true);
        gateway_task
            .await
            .map_err(anyhow::Error::from)
            .and_then(|result| result)
    };
    if let Err(error) = finish_ordered_shutdown(&shutdown_order, cleanup, gateway_shutdown).await {
        tracing::error!(error = %error, "Core ordered shutdown did not complete cleanly");
        failure.get_or_insert_with(|| anyhow::anyhow!(error.to_string()));
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    while !*shutdown.borrow() && shutdown.changed().await.is_ok() {}
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
