#![forbid(unsafe_code)]

mod redaction;

use std::path::Path;

pub use redaction::{redact_json, redact_text};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub struct LoggingGuard {
    _file_guard: WorkerGuard,
}

pub fn init_logging(log_dir: &Path, filter: &str) -> anyhow::Result<LoggingGuard> {
    std::fs::create_dir_all(log_dir)?;

    let file_appender = tracing_appender::rolling::daily(log_dir, "audiodown.jsonl");
    let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);
    let filter = EnvFilter::try_new(filter)?;

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(std::io::stdout);
    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_writer(file_writer);

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .try_init()?;

    Ok(LoggingGuard {
        _file_guard: file_guard,
    })
}
