#![forbid(unsafe_code)]

#[tokio::main]
async fn main() -> Result<(), audiodown_proxy_gateway::GatewayError> {
    audiodown_proxy_gateway::run().await
}
