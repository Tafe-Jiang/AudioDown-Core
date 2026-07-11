use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use secrecy::SecretString;

pub const DEFAULT_GITHUB_API_BASE: &str = "https://api.github.com";
pub const DEFAULT_GITHUB_ARCHIVE_BASE: &str = "https://codeload.github.com";

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub database_url: String,
    pub supervisor_socket: PathBuf,
    pub core_token_file: PathBuf,
    pub log_filter: String,
    pub dev_mode: bool,
    pub dev_token: Option<SecretString>,
    pub github_api_base: String,
    pub github_archive_base: String,
    pub plugin_reconcile_interval: Duration,
    pub plugin_idle_timeout: Duration,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = env_value("AUDIODOWN_BIND", "0.0.0.0:18080").parse()?;
        let data_dir = PathBuf::from(env_value("AUDIODOWN_DATA_DIR", "/data"));
        let database_url =
            env::var("AUDIODOWN_DATABASE_URL").unwrap_or_else(|_| default_database_url(&data_dir));
        let dev_mode = env_flag("AUDIODOWN_DEV_MODE");
        let github_api_base = env_value("AUDIODOWN_GITHUB_API_BASE", DEFAULT_GITHUB_API_BASE);
        let github_archive_base =
            env_value("AUDIODOWN_GITHUB_ARCHIVE_BASE", DEFAULT_GITHUB_ARCHIVE_BASE);
        let plugin_reconcile_interval = Self::validate_lifecycle_seconds(
            env_value("AUDIODOWN_PLUGIN_RECONCILE_SECONDS", "30").parse()?,
            dev_mode,
        )?;
        let plugin_idle_timeout = Self::validate_lifecycle_seconds(
            env_value("AUDIODOWN_PLUGIN_IDLE_TIMEOUT_SECONDS", "900").parse()?,
            dev_mode,
        )?;
        if !dev_mode
            && (github_api_base != DEFAULT_GITHUB_API_BASE
                || github_archive_base != DEFAULT_GITHUB_ARCHIVE_BASE)
        {
            anyhow::bail!("custom GitHub service bases require AUDIODOWN_DEV_MODE=1");
        }

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
            dev_mode,
            dev_token: env::var("AUDIODOWN_DEV_TOKEN")
                .ok()
                .filter(|value| !value.is_empty())
                .map(SecretString::new),
            github_api_base,
            github_archive_base,
            plugin_reconcile_interval,
            plugin_idle_timeout,
        })
    }

    pub fn for_test_with_dev_token(token: &str) -> Self {
        Self {
            bind: "127.0.0.1:18080".parse().expect("test bind must parse"),
            data_dir: PathBuf::from("/tmp/audiodown-test"),
            database_url: "sqlite::memory:".to_string(),
            supervisor_socket: PathBuf::from("/tmp/audiodown-supervisor.sock"),
            core_token_file: PathBuf::from("/tmp/audiodown-core.token"),
            log_filter: "info".to_string(),
            dev_mode: true,
            dev_token: Some(SecretString::new(token.to_string())),
            github_api_base: DEFAULT_GITHUB_API_BASE.to_string(),
            github_archive_base: DEFAULT_GITHUB_ARCHIVE_BASE.to_string(),
            plugin_reconcile_interval: Duration::from_secs(30),
            plugin_idle_timeout: Duration::from_secs(900),
        }
    }

    pub fn validate_lifecycle_seconds(seconds: u64, dev_mode: bool) -> anyhow::Result<Duration> {
        if seconds == 0 {
            anyhow::bail!("plugin lifecycle intervals must be positive");
        }
        if seconds < 5 && !dev_mode {
            anyhow::bail!("plugin lifecycle intervals below five seconds require developer mode");
        }
        Ok(Duration::from_secs(seconds))
    }
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
        .unwrap_or(false)
}

fn default_database_url(data_dir: &Path) -> String {
    format!("sqlite://{}/audiodown.db", data_dir.display())
}
