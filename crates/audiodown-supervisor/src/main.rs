#![forbid(unsafe_code)]

use audiodown_supervisor::{
    build_proxy,
    config::{ensure_identity, Config},
    docker::DockerAdapter,
    server,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("build-proxy") => build_proxy::run().await,
        Some(_) => anyhow::bail!("unknown Supervisor mode"),
        None => run_supervisor().await,
    }
}

async fn run_supervisor() -> anyhow::Result<()> {
    let config = Config::from_env();
    let identity = ensure_identity(&config).await?;
    let docker = DockerAdapter::connect(identity.installation_id.clone())?;
    server::run(config, identity, docker).await
}
