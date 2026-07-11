use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub database_url: String,
    pub supervisor_socket: PathBuf,
    pub core_token_file: PathBuf,
    pub log_filter: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = env_value("AUDIODOWN_BIND", "0.0.0.0:18080").parse()?;
        let data_dir = PathBuf::from(env_value("AUDIODOWN_DATA_DIR", "/data"));
        let database_url = env::var("AUDIODOWN_DATABASE_URL")
            .unwrap_or_else(|_| default_database_url(&data_dir));

        Ok(Self {
            bind,
            data_dir,
            database_url,
            supervisor_socket: PathBuf::from(env_value(
                "AUDIODOWN_SUPERVISOR_SOCKET",
                "/run/audiodown/supervisor.sock",
            )),
            core_token_file: PathBuf::from(env_value(
                "AUDIODOWN_CORE_TOKEN_FILE",
                "/run/audiodown/core.token",
            )),
            log_filter: env_value("AUDIODOWN_LOG", "info"),
        })
    }
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn default_database_url(data_dir: &Path) -> String {
    format!("sqlite://{}/audiodown.db", data_dir.display())
}
