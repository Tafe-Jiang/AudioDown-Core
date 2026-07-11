use std::{
    collections::HashMap,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use audiodown_domain::plugin::PluginStatus;
use chrono::Utc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

use crate::{
    config::{Config, SupervisorIdentity},
    docker::DockerAdapter,
    install_record,
    protocol::{SupervisorMethod, SupervisorRequest, SupervisorResponse},
};

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_CLOCK_SKEW_SECONDS: i64 = 30;
const NONCE_RETENTION: Duration = Duration::from_secs(120);

pub async fn run(
    config: Config,
    identity: SupervisorIdentity,
    docker: DockerAdapter,
) -> anyhow::Result<()> {
    prepare_socket(&config.socket_path).await?;
    let listener = UnixListener::bind(&config.socket_path)?;
    set_socket_permissions(&config.socket_path).await?;
    let authenticator = std::sync::Arc::new(Authenticator::new(identity.token));
    let runtime = std::sync::Arc::new(Runtime {
        docker,
        plugin_data: config.plugin_data,
        installation_id: identity.installation_id,
    });

    loop {
        let (stream, _) = listener.accept().await?;
        let authenticator = authenticator.clone();
        let runtime = runtime.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, authenticator, runtime).await {
                tracing::warn!(error = %error, "Supervisor request failed");
            }
        });
    }
}

async fn handle_connection(
    mut stream: UnixStream,
    authenticator: std::sync::Arc<Authenticator>,
    runtime: std::sync::Arc<Runtime>,
) -> anyhow::Result<()> {
    let bytes = read_request(&mut stream).await?;
    let request = match serde_json::from_slice::<SupervisorRequest>(&bytes) {
        Ok(request) => request,
        Err(error) => {
            write_response(
                &mut stream,
                &SupervisorResponse::failure(
                    "",
                    "INVALID_REQUEST",
                    format!("Malformed request: {error}"),
                ),
            )
            .await?;
            return Ok(());
        }
    };

    let response = match request.validate_shape() {
        Err(error) => SupervisorResponse::failure(request.id, "INVALID_REQUEST", error.to_string()),
        Ok(()) => match authenticator.validate(&request) {
            Err(error) => {
                SupervisorResponse::failure(request.id, "UNAUTHORIZED", error.to_string())
            }
            Ok(()) => dispatch(request, runtime).await,
        },
    };
    write_response(&mut stream, &response).await?;
    Ok(())
}

async fn dispatch(
    request: SupervisorRequest,
    runtime: std::sync::Arc<Runtime>,
) -> SupervisorResponse {
    match request.method {
        SupervisorMethod::SystemPing => SupervisorResponse::success(
            request.id,
            serde_json::json!({"ok": true, "service": "audiodown-supervisor"}),
        ),
        SupervisorMethod::PluginInspect => {
            let plugin_id = request
                .params
                .expect("validated request has params")
                .plugin_id;
            match runtime.docker.inspect_plugin(&plugin_id).await {
                Ok(inspection) => SupervisorResponse::success(
                    request.id,
                    serde_json::json!({
                        "pluginId": plugin_id,
                        "status": if inspection.running {
                            PluginStatus::Healthy
                        } else {
                            PluginStatus::Stopped
                        },
                        "containerId": inspection.container_id
                    }),
                ),
                Err(error) => docker_failure(request.id, error),
            }
        }
        SupervisorMethod::PluginStart => {
            let plugin_id = request
                .params
                .expect("validated request has params")
                .plugin_id;
            let install = match install_record::load(
                &runtime.plugin_data,
                &runtime.installation_id,
                &plugin_id,
            )
            .await
            {
                Ok(install) => install,
                Err(error) => {
                    return SupervisorResponse::failure(
                        request.id,
                        "INVALID_INSTALL_RECORD",
                        error.to_string(),
                    )
                }
            };
            match runtime.docker.start_plugin(install).await {
                Ok(started) => SupervisorResponse::success(
                    request.id,
                    serde_json::json!({
                        "pluginId": plugin_id,
                        "status": PluginStatus::Healthy,
                        "containerId": started.container_id,
                        "logs": started.logs
                    }),
                ),
                Err(crate::docker::DockerAdapterError::Handshake(error)) => {
                    SupervisorResponse::failure(request.id, "PLUGIN_NOT_COMPATIBLE", error)
                }
                Err(error) => docker_failure(request.id, error),
            }
        }
        SupervisorMethod::PluginStop => {
            let plugin_id = request
                .params
                .expect("validated request has params")
                .plugin_id;
            match runtime.docker.stop_plugin(&plugin_id).await {
                Ok(container_id) => SupervisorResponse::success(
                    request.id,
                    serde_json::json!({
                        "pluginId": plugin_id,
                        "status": PluginStatus::Stopped,
                        "containerId": container_id
                    }),
                ),
                Err(error) => docker_failure(request.id, error),
            }
        }
        SupervisorMethod::PluginLogs => {
            let plugin_id = request
                .params
                .expect("validated request has params")
                .plugin_id;
            match runtime.docker.plugin_logs(&plugin_id).await {
                Ok(logs) => SupervisorResponse::success(
                    request.id,
                    serde_json::json!({
                        "pluginId": plugin_id,
                        "logs": logs
                    }),
                ),
                Err(error) => docker_failure(request.id, error),
            }
        }
    }
}

fn docker_failure(
    request_id: String,
    error: crate::docker::DockerAdapterError,
) -> SupervisorResponse {
    SupervisorResponse::failure(request_id, "DOCKER_OPERATION_FAILED", error.to_string())
}

struct Runtime {
    docker: DockerAdapter,
    plugin_data: std::path::PathBuf,
    installation_id: String,
}

async fn read_request(stream: &mut UnixStream) -> anyhow::Result<Vec<u8>> {
    let mut request = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    loop {
        let read = stream.read(&mut byte).await?;
        if read == 0 {
            break;
        }
        if byte[0] == b'\n' {
            break;
        }
        request.push(byte[0]);
        if request.len() > MAX_REQUEST_BYTES {
            anyhow::bail!("request exceeds 64 KiB limit");
        }
    }
    Ok(request)
}

async fn write_response(
    stream: &mut UnixStream,
    response: &SupervisorResponse,
) -> anyhow::Result<()> {
    let mut encoded = serde_json::to_vec(response)?;
    encoded.push(b'\n');
    stream.write_all(&encoded).await?;
    stream.shutdown().await?;
    Ok(())
}

async fn prepare_socket(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn set_socket_permissions(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666)).await?;
    }
    Ok(())
}

struct Authenticator {
    token: String,
    nonces: Mutex<HashMap<String, Instant>>,
}

impl Authenticator {
    fn new(token: String) -> Self {
        Self {
            token,
            nonces: Mutex::new(HashMap::new()),
        }
    }

    fn validate(&self, request: &SupervisorRequest) -> Result<(), AuthError> {
        if request.token != self.token {
            return Err(AuthError::InvalidToken);
        }
        if request.nonce.is_empty() {
            return Err(AuthError::InvalidNonce);
        }
        let now = Utc::now().timestamp();
        if (now - request.timestamp).abs() > MAX_CLOCK_SKEW_SECONDS {
            return Err(AuthError::StaleTimestamp);
        }

        let mut nonces = self.nonces.lock().map_err(|_| AuthError::StatePoisoned)?;
        nonces.retain(|_, inserted_at| inserted_at.elapsed() < NONCE_RETENTION);
        if nonces.contains_key(&request.nonce) {
            return Err(AuthError::DuplicateNonce);
        }
        nonces.insert(request.nonce.clone(), Instant::now());
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
enum AuthError {
    #[error("invalid authentication token")]
    InvalidToken,
    #[error("request timestamp is outside the allowed window")]
    StaleTimestamp,
    #[error("request nonce is invalid")]
    InvalidNonce,
    #[error("request nonce was already used")]
    DuplicateNonce,
    #[error("authentication state is unavailable")]
    StatePoisoned,
}
