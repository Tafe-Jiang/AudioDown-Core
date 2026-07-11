#![forbid(unsafe_code)]

use audiodown_plugin_manager::{github::GitHubClient, service::PluginManagerService};
use audiodown_server::{
    app::build_router, config::Config, plugin_manager_adapters::SqlitePluginManagerStore,
    state::AppState, supervisor::UnixSupervisorClient,
};
use audiodown_storage::Storage;
use tokio::net::TcpListener;

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

    let repository_source = std::sync::Arc::new(GitHubClient::new(
        &config.github_api_base,
        &config.github_archive_base,
    )?);
    let plugin_manager = std::sync::Arc::new(PluginManagerService::new(
        std::sync::Arc::new(SqlitePluginManagerStore::new(storage.clone())),
        repository_source,
        config.data_dir.join("plugins"),
        semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        semver::Version::new(1, 0, 0),
    ));
    let state = AppState::new(
        storage,
        semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        std::sync::Arc::new(UnixSupervisorClient::new(
            &config.supervisor_socket,
            &config.core_token_file,
        )),
    )
    .with_plugin_manager(plugin_manager)
    .with_development(config.dev_mode, config.dev_token);
    let app = build_router(state);
    let listener = TcpListener::bind(config.bind).await?;
    tracing::info!(address = %config.bind, "AudioDown Core listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
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
