use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use audiodown_server::{
    config::Config,
    lifecycle::{run_lifecycle_reconciler, LifecycleManager},
};
use chrono::{DateTime, Utc};
use tokio::sync::watch;

#[tokio::test(start_paused = true)]
async fn wakes_only_on_the_interval_and_stops_through_cancellation() {
    let manager = Arc::new(FakeManager::default());
    let (cancel, receiver) = watch::channel(false);
    let task = tokio::spawn(run_lifecycle_reconciler(
        manager.clone(),
        Duration::from_secs(30),
        Duration::from_secs(900),
        receiver,
    ));

    tokio::task::yield_now().await;
    assert_eq!(manager.calls(), 0);
    tokio::time::advance(Duration::from_secs(29)).await;
    tokio::task::yield_now().await;
    assert_eq!(manager.calls(), 0);
    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(manager.calls(), 1);
    assert_eq!(manager.idle_timeouts(), vec![Duration::from_secs(900)]);

    cancel.send(true).unwrap();
    tokio::task::yield_now().await;
    task.await.unwrap();
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(manager.calls(), 1);
}

#[test]
fn lifecycle_defaults_and_developer_only_short_intervals_are_enforced() {
    let config = Config::for_test_with_dev_token("hidden");
    assert_eq!(config.plugin_reconcile_interval, Duration::from_secs(30));
    assert_eq!(config.plugin_idle_timeout, Duration::from_secs(900));
    assert!(Config::validate_lifecycle_seconds(4, false).is_err());
    assert_eq!(
        Config::validate_lifecycle_seconds(4, true).unwrap(),
        Duration::from_secs(4)
    );
    assert_eq!(
        Config::validate_lifecycle_seconds(5, false).unwrap(),
        Duration::from_secs(5)
    );
}

#[test]
fn scheduler_has_no_storage_route_or_supervisor_logic() {
    let source = include_str!("../src/lifecycle.rs");
    assert!(!source.contains("audiodown_storage"));
    assert!(!source.contains("routes::"));
    assert!(!source.contains("SupervisorClient"));
    assert!(!source.contains(".storage"));
    assert!(!source.contains(".supervisor"));
}

#[derive(Default)]
struct FakeManager {
    calls: AtomicUsize,
    idle_timeouts: Mutex<Vec<Duration>>,
}

impl FakeManager {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn idle_timeouts(&self) -> Vec<Duration> {
        self.idle_timeouts.lock().unwrap().clone()
    }
}

#[async_trait]
impl LifecycleManager for FakeManager {
    async fn reconcile_due_plugins(
        &self,
        _now: DateTime<Utc>,
        idle_timeout: Duration,
    ) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.idle_timeouts.lock().unwrap().push(idle_timeout);
        Ok(())
    }
}
