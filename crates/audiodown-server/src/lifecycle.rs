use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use audiodown_plugin_manager::service::PluginManagerService;
use chrono::{DateTime, Utc};
use tokio::sync::watch;

#[async_trait]
pub trait LifecycleManager: Send + Sync {
    async fn reconcile_due_plugins(
        &self,
        now: DateTime<Utc>,
        idle_timeout: Duration,
    ) -> Result<(), String>;
}

#[async_trait]
impl LifecycleManager for PluginManagerService {
    async fn reconcile_due_plugins(
        &self,
        now: DateTime<Utc>,
        idle_timeout: Duration,
    ) -> Result<(), String> {
        PluginManagerService::reconcile_due_plugins(self, now, idle_timeout)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub async fn run_lifecycle_reconciler(
    manager: Arc<dyn LifecycleManager>,
    interval: Duration,
    idle_timeout: Duration,
    mut cancellation: watch::Receiver<bool>,
) {
    loop {
        if *cancellation.borrow() {
            return;
        }
        tokio::select! {
            changed = cancellation.changed() => {
                if changed.is_err() || *cancellation.borrow() {
                    return;
                }
            }
            () = tokio::time::sleep(interval) => {
                if let Err(error) = manager.reconcile_due_plugins(Utc::now(), idle_timeout).await {
                    tracing::warn!(error = %error, "Plugin lifecycle reconciliation failed");
                }
            }
        }
    }
}
