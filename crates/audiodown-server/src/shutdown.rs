use std::sync::{Arc, Mutex};

use tokio::task::{JoinHandle, JoinSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownPhase {
    WorkQuiesced,
    RuntimeCleanupFinished,
    GatewayShutdownStarted,
}

pub enum CoreTaskExit {
    Work {
        name: &'static str,
        result: anyhow::Result<()>,
    },
    Gateway {
        result: anyhow::Result<()>,
    },
    WorkJoinFailed(anyhow::Error),
    GatewayJoinFailed(anyhow::Error),
    WorkSetEmpty,
}

#[derive(Clone, Default)]
pub struct ShutdownOrder {
    phases: Arc<Mutex<Vec<ShutdownPhase>>>,
}

impl ShutdownOrder {
    pub fn record(&self, phase: ShutdownPhase) {
        if let Ok(mut phases) = self.phases.lock() {
            phases.push(phase);
        }
    }

    pub fn phases(&self) -> Vec<ShutdownPhase> {
        self.phases
            .lock()
            .map_or_else(|_| Vec::new(), |phases| phases.clone())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OrderedShutdownError<CleanupError, GatewayError> {
    #[error("runtime cleanup failed before gateway shutdown")]
    Cleanup(CleanupError),
    #[error("proxy gateway shutdown failed after runtime cleanup")]
    Gateway(GatewayError),
}

pub async fn wait_for_core_task_exit(
    work_tasks: &mut JoinSet<(&'static str, anyhow::Result<()>)>,
    gateway_task: &mut JoinHandle<anyhow::Result<()>>,
) -> CoreTaskExit {
    tokio::select! {
        completed = work_tasks.join_next() => {
            match completed {
                Some(Ok((name, result))) => CoreTaskExit::Work { name, result },
                Some(Err(error)) => CoreTaskExit::WorkJoinFailed(
                    anyhow::Error::new(error).context("failed to join Core work task")
                ),
                None => CoreTaskExit::WorkSetEmpty,
            }
        }
        completed = gateway_task => {
            match completed {
                Ok(result) => CoreTaskExit::Gateway { result },
                Err(error) => CoreTaskExit::GatewayJoinFailed(
                    anyhow::Error::new(error).context("failed to join proxy gateway task")
                ),
            }
        }
    }
}

pub async fn finish_ordered_shutdown<Cleanup, Gateway, CleanupError, GatewayError>(
    order: &ShutdownOrder,
    cleanup: Cleanup,
    gateway_shutdown: Gateway,
) -> Result<(), OrderedShutdownError<CleanupError, GatewayError>>
where
    Cleanup: std::future::Future<Output = Result<(), CleanupError>>,
    Gateway: std::future::Future<Output = Result<(), GatewayError>>,
{
    let cleanup_result = cleanup.await;
    order.record(ShutdownPhase::RuntimeCleanupFinished);
    order.record(ShutdownPhase::GatewayShutdownStarted);
    let gateway_result = gateway_shutdown.await;

    match (cleanup_result, gateway_result) {
        (Err(error), _) => Err(OrderedShutdownError::Cleanup(error)),
        (Ok(()), Err(error)) => Err(OrderedShutdownError::Gateway(error)),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        finish_ordered_shutdown, wait_for_core_task_exit, CoreTaskExit, ShutdownOrder,
        ShutdownPhase,
    };
    use tokio::task::JoinSet;

    #[tokio::test]
    async fn runtime_cleanup_finishes_before_registry_gateway_shutdown() {
        let order = ShutdownOrder::default();
        order.record(ShutdownPhase::WorkQuiesced);
        let gateway_order = order.clone();

        finish_ordered_shutdown(&order, async move { Ok::<_, ()>(()) }, async move {
            assert_eq!(
                gateway_order.phases(),
                vec![
                    ShutdownPhase::WorkQuiesced,
                    ShutdownPhase::RuntimeCleanupFinished,
                    ShutdownPhase::GatewayShutdownStarted,
                ]
            );
            Ok::<_, ()>(())
        })
        .await
        .unwrap();

        assert_eq!(
            order.phases(),
            vec![
                ShutdownPhase::WorkQuiesced,
                ShutdownPhase::RuntimeCleanupFinished,
                ShutdownPhase::GatewayShutdownStarted,
            ]
        );
    }

    #[tokio::test]
    async fn gateway_early_exit_is_observed_before_work_tasks() {
        let mut work_tasks = JoinSet::new();
        work_tasks.spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            ("HTTP server", Ok::<(), anyhow::Error>(()))
        });
        let mut gateway_task = tokio::spawn(async { Ok::<(), anyhow::Error>(()) });

        match wait_for_core_task_exit(&mut work_tasks, &mut gateway_task).await {
            CoreTaskExit::Gateway { result } => assert!(result.is_ok()),
            CoreTaskExit::Work { .. } => panic!("gateway exit should win"),
            CoreTaskExit::WorkJoinFailed(_)
            | CoreTaskExit::GatewayJoinFailed(_)
            | CoreTaskExit::WorkSetEmpty => panic!("gateway exit should be reported directly"),
        }

        work_tasks.abort_all();
        while work_tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn gateway_join_failure_is_reported_without_repolling_the_handle() {
        let mut work_tasks = JoinSet::new();
        work_tasks.spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            ("HTTP server", Ok::<(), anyhow::Error>(()))
        });
        let mut gateway_task = tokio::spawn(async move {
            panic!("gateway task failed");
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        });

        assert!(matches!(
            wait_for_core_task_exit(&mut work_tasks, &mut gateway_task).await,
            CoreTaskExit::GatewayJoinFailed(_)
        ));

        work_tasks.abort_all();
        while work_tasks.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn work_join_failure_is_reported_for_ordered_cleanup() {
        let mut work_tasks = JoinSet::new();
        work_tasks.spawn(async move {
            panic!("work task failed");
            #[allow(unreachable_code)]
            ("HTTP server", Ok::<(), anyhow::Error>(()))
        });
        let mut gateway_task = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok::<(), anyhow::Error>(())
        });

        assert!(matches!(
            wait_for_core_task_exit(&mut work_tasks, &mut gateway_task).await,
            CoreTaskExit::WorkJoinFailed(_)
        ));

        gateway_task.abort();
        let _ = gateway_task.await;
    }
}
