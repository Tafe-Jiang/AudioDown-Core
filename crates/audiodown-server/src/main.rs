#![forbid(unsafe_code)]

use audiodown_plugin_manager::{github::GitHubClient, service::PluginManagerService};
use audiodown_server::{
    app::build_router,
    config::Config,
    lifecycle::run_lifecycle_reconciler,
    plugin_manager_adapters::{
        ConfiguredLifecycleRiskAuthorizer, SqlitePluginManagerStore, SupervisorPluginRuntime,
    },
    state::{AppState, DevelopmentConfig},
    supervisor::UnixSupervisorClient,
};
use audiodown_storage::Storage;
use tokio::{net::TcpListener, sync::watch};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    tokio::fs::create_dir_all(&config.data_dir).await?;
    tokio::fs::create_dir_all(config.data_dir.join("logs")).await?;
    tokio::fs::create_dir_all(config.data_dir.join("plugins")).await?;

    let _logging_guard =
        audiodown_logging::init_logging(&config.data_dir.join("logs"), &config.log_filter)?;
    let storage = Storage::connect(&config.database_url).await?;
    storage.migrate().await?;

    let supervisor = std::sync::Arc::new(UnixSupervisorClient::new(
        &config.supervisor_socket,
        &config.core_token_file,
    ));
    let development = DevelopmentConfig {
        enabled: config.dev_mode,
        token: config.dev_token,
    };
    let repository_source = std::sync::Arc::new(GitHubClient::new(
        &config.github_api_base,
        &config.github_archive_base,
    )?);
    let plugin_manager = std::sync::Arc::new(
        PluginManagerService::new(
            std::sync::Arc::new(SqlitePluginManagerStore::new(storage.clone())),
            repository_source,
            config.data_dir.join("plugins"),
            semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
            semver::Version::new(1, 0, 0),
        )
        .with_installation_ports(
            std::sync::Arc::new(SupervisorPluginRuntime::new(supervisor.clone())),
            std::sync::Arc::new(ConfiguredLifecycleRiskAuthorizer::new(development.clone())),
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
    .with_development(development.enabled, development.token);
    let app = build_router(state);
    let listener = TcpListener::bind(config.bind).await?;
    tracing::info!(address = %config.bind, "AudioDown Core listening");

    let (lifecycle_cancel, lifecycle_receiver) = watch::channel(false);
    let lifecycle_task = tokio::spawn(run_lifecycle_reconciler(
        lifecycle_manager,
        config.plugin_reconcile_interval,
        config.plugin_idle_timeout,
        lifecycle_receiver,
    ));
    let shutdown_cancel = lifecycle_cancel.clone();
    let shutdown = async move {
        shutdown_signal().await;
        let _ = shutdown_cancel.send(true);
    };
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    let _ = lifecycle_cancel.send(true);
    let _ = lifecycle_task.await;
    serve_result?;
    Ok(())
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
