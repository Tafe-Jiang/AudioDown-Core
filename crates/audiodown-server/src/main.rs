#![forbid(unsafe_code)]

use audiodown_server::{
    app::build_router,
    config::Config,
    state::{AppState, UnavailableSupervisorClient},
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

    let state = AppState::new(
        storage,
        semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        std::sync::Arc::new(UnavailableSupervisorClient),
    );
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
