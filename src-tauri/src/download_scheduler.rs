use std::{future::Future, sync::Arc};

use parking_lot::Mutex;
use tokio::{sync::watch, task::JoinSet};

/// 降低上限时让在途任务自然结束；新任务始终使用同一份额度。
pub struct DownloadLimiter {
    state: Mutex<LimitState>,
    changed: watch::Sender<()>,
}

struct LimitState {
    limit: usize,
    active: usize,
}

pub struct DownloadPermit(Arc<DownloadLimiter>);

impl DownloadLimiter {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(LimitState {
                limit: limit.max(1),
                active: 0,
            }),
            changed: watch::channel(()).0,
        })
    }

    pub fn set_limit(&self, limit: usize) {
        self.state.lock().limit = limit.max(1);
        self.changed.send_replace(());
    }

    pub async fn acquire(self: &Arc<Self>) -> DownloadPermit {
        let mut changed = self.changed.subscribe();
        loop {
            {
                let mut state = self.state.lock();
                if state.active < state.limit {
                    state.active += 1;
                    return DownloadPermit(self.clone());
                }
            }
            let _ = changed.changed().await;
        }
    }
}

impl Drop for DownloadPermit {
    fn drop(&mut self) {
        self.0.state.lock().active -= 1;
        self.0.changed.send_replace(());
    }
}

/// 只保留有限数量的在途任务，每完成一个便补充一个。
pub async fn run_bounded<I, F, Fut>(items: I, limit: usize, mut run: F) -> anyhow::Result<()>
where
    I: IntoIterator,
    F: FnMut(I::Item) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut items = items.into_iter();
    let mut pending = JoinSet::new();
    loop {
        while pending.len() < limit.max(1) {
            let Some(item) = items.next() else { break };
            pending.spawn(run(item));
        }
        match pending.join_next().await {
            Some(result) => result?,
            None => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn lowering_limit_waits_for_existing_permits_and_raising_wakes_waiter() {
        let limiter = DownloadLimiter::new(2);
        let first = limiter.acquire().await;
        let second = limiter.acquire().await;
        limiter.set_limit(1);
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_millis(30), limiter.acquire())
                .await
                .is_err()
        );
        let waiting_limiter = limiter.clone();
        let waiter = tokio::spawn(async move { waiting_limiter.acquire().await });
        limiter.set_limit(2);
        let third = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        drop(second);
        drop(third);
        assert_eq!(limiter.state.lock().active, 0);
    }

    #[tokio::test]
    async fn slow_first_image_does_not_block_replenishing_other_slot() {
        let (started, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (release, hold) = tokio::sync::oneshot::channel::<()>();
        let mut hold = Some(hold);
        let worker = tokio::spawn(async move {
            run_bounded(0..3, 2, |index| {
                let started = started.clone();
                let hold = if index == 0 { hold.take() } else { None };
                async move {
                    started.send(index).unwrap();
                    if let Some(hold) = hold {
                        let _ = hold.await;
                    }
                }
            })
            .await
            .unwrap();
        });
        let mut indices = Vec::new();
        for _ in 0..3 {
            indices.push(
                tokio::time::timeout(Duration::from_secs(1), received.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        indices.sort_unstable();
        assert_eq!(indices, [0, 1, 2]);
        release.send(()).unwrap();
        worker.await.unwrap();
    }
}
