#![forbid(unsafe_code)]

use audiodown_supervisor::{
    config::{ensure_identity, Config},
    docker::DockerAdapter,
    server,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env();
    let identity = ensure_identity(&config).await?;
    let docker = DockerAdapter::connect(identity.installation_id.clone())?;
    server::run(config, identity, docker).await
}
