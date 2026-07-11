use std::{
    env,
    path::{Path, PathBuf},
};

use tokio::io::AsyncWriteExt;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Config {
    pub socket_path: PathBuf,
    pub plugin_data: PathBuf,
    pub installation_id_file: PathBuf,
    pub core_token_file: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            socket_path: env_path(
                "AUDIODOWN_SUPERVISOR_SOCKET",
                "/run/audiodown/supervisor.sock",
            ),
            plugin_data: env_path("AUDIODOWN_PLUGIN_DATA", "/data/plugins"),
            installation_id_file: env_path(
                "AUDIODOWN_INSTALLATION_ID_FILE",
                "/data/plugins/installation-id",
            ),
            core_token_file: env_path("AUDIODOWN_CORE_TOKEN_FILE", "/run/audiodown/core.token"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SupervisorIdentity {
    pub installation_id: String,
    pub token: String,
}

pub async fn ensure_identity(config: &Config) -> anyhow::Result<SupervisorIdentity> {
    tokio::fs::create_dir_all(&config.plugin_data).await?;
    let installation_id =
        ensure_secret_file(&config.installation_id_file, || Uuid::new_v4().to_string()).await?;
    let token = ensure_secret_file(&config.core_token_file, || {
        format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
    })
    .await?;
    Ok(SupervisorIdentity {
        installation_id,
        token,
    })
}

async fn ensure_secret_file(
    path: &Path,
    generate: impl FnOnce() -> String,
) -> anyhow::Result<String> {
    if let Ok(existing) = tokio::fs::read_to_string(path).await {
        let existing = existing.trim().to_string();
        if existing.is_empty() {
            anyhow::bail!("identity file is empty: {}", path.display());
        }
        set_mode_0600(path).await?;
        return Ok(existing);
    }

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let value = generate();
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).await?;
    file.write_all(value.as_bytes()).await?;
    file.write_all(b"\n").await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temporary, path).await?;
    set_mode_0600(path).await?;
    Ok(value)
}

async fn set_mode_0600(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

fn env_path(name: &str, default: &str) -> PathBuf {
    PathBuf::from(env::var(name).unwrap_or_else(|_| default.to_string()))
}
