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
            Arc::new(SupervisorPluginRuntime::new(supervisor.clone())),
            Arc::new(ConfiguredLifecycleRiskAuthorizer::new(development.clone())),
        ),
    );
    if let Err(error) = plugin_manager.reconcile_install_operations().await {
        tracing::warn!(error = %error, "Plugin install reconciliation will retry on next startup");
    }
    let lifecycle_manager = plugin_manager.clone();
    let state = AppState::new(
        storage,
        semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        supervisor,
    )
    .with_plugin_manager(plugin_manager)
    .with_development(development.enabled, development.token)
    .with_proxy_runtime(proxy_runtime);
    let app = build_router(state);
    let listener = TcpListener::bind(config.bind).await?;
    tracing::info!(address = %config.bind, "AudioDown Core listening");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut tasks = JoinSet::new();

    let gateway_shutdown = shutdown_rx.clone();
    tasks.spawn(async move {
        (
            "proxy gateway",
            proxy_gateway
                .run(gateway_shutdown)
                .await
                .map_err(anyhow::Error::from),
        )
    });

    let lifecycle_shutdown = shutdown_rx.clone();
    tasks.spawn(async move {
        run_lifecycle_reconciler(
            lifecycle_manager,
            config.plugin_reconcile_interval,
            config.plugin_idle_timeout,
            lifecycle_shutdown,
        )
        .await;
        ("lifecycle reconciler", Ok::<(), anyhow::Error>(()))
    });

    let server_shutdown = shutdown_rx;
    tasks.spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(wait_for_shutdown(server_shutdown))
            .await
            .map_err(anyhow::Error::from);
        ("HTTP server", result)
    });

    let mut early_exit = None;
    tokio::select! {
        () = shutdown_signal() => {}
        completed = tasks.join_next() => {
            early_exit = Some(completed.ok_or_else(|| anyhow::anyhow!("Core task set was empty"))??);
        }
    }
    let _ = shutdown_tx.send(true);

    let mut failure = None;
    if let Some((name, result)) = early_exit {
        failure = Some(match result {
            Ok(()) => anyhow::anyhow!("{name} exited unexpectedly"),
            Err(error) => error.context(format!("{name} failed")),
        });
    }
    while let Some(completed) = tasks.join_next().await {
        let (name, result) = completed?;
        if let Err(error) = result {
            failure.get_or_insert_with(|| error.context(format!("{name} failed")));
        }
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
