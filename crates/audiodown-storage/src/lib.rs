#![forbid(unsafe_code)]

mod log_repository;
mod plugin_repository;
mod risk_grant_repository;

use std::{str::FromStr, time::Duration};

pub use log_repository::{LogFilter, LogRepository};
pub use plugin_repository::{PluginRecord, PluginRepository};
pub use risk_grant_repository::{RiskGrantRecord, RiskGrantRepository};
use sqlx::{
    migrate::MigrateError,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use thiserror::Error;

#[derive(Clone)]
pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        let in_memory = url.contains(":memory:");
        let mut options = SqliteConnectOptions::from_str(url)?
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5))
            .shared_cache(in_memory)
            .create_if_missing(!in_memory);

        if !in_memory {
            options = options.journal_mode(SqliteJournalMode::Wal);
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<(), StorageError> {
        sqlx::migrate!("../../migrations").run(&self.pool).await?;
        Ok(())
    }

    pub fn plugins(&self) -> PluginRepository<'_> {
        PluginRepository::new(&self.pool)
    }

    pub fn logs(&self) -> LogRepository<'_> {
        LogRepository::new(&self.pool)
    }

    pub fn risk_grants(&self) -> RiskGrantRepository<'_> {
        RiskGrantRepository::new(&self.pool)
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] MigrateError),
    #[error("invalid stored data: {0}")]
    InvalidData(String),
    #[error("record not found")]
    NotFound,
}
