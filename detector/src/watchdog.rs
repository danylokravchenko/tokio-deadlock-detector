use std::{sync::Arc, time::Duration};

use crate::{
    graph::{GRAPH, Node},
    monitor::{REGISTRY, TaskInfo},
};
use tokio::task::JoinHandle;

pub struct Watchdog {
    handle: JoinHandle<()>,
}

impl Watchdog {
    pub fn stop(self) {
        self.handle.abort();
    }
}

pub struct WatchdogBuilder {
    stall_threshold_ms: u64,
    sample_interval_ms: u64,
    on_stall: Option<Arc<dyn Fn(Vec<TaskInfo>) + Send + Sync>>,
    #[cfg(feature = "deadlock")]
    on_deadlock: Option<Arc<dyn Fn(Vec<TaskInfo>, Vec<Node>) + Send + Sync>>,
}

impl Default for WatchdogBuilder {
    fn default() -> Self {
        Self {
            stall_threshold_ms: 100,
            sample_interval_ms: 50,
            on_stall: None,
            #[cfg(feature = "deadlock")]
            on_deadlock: None,
        }
    }
}

impl WatchdogBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stall_threshold_ms(mut self, ms: u64) -> Self {
        self.stall_threshold_ms = ms;
        self
    }

    pub fn sample_interval_ms(mut self, ms: u64) -> Self {
        self.sample_interval_ms = ms;
        self
    }

    pub fn on_stall<F>(mut self, f: F) -> Self
    where
        F: Fn(Vec<TaskInfo>) + Send + Sync + 'static,
    {
        self.on_stall = Some(Arc::new(f));
        self
    }

    #[cfg(feature = "deadlock")]
    pub fn on_deadlock<F>(mut self, f: F) -> Self
    where
        F: Fn(Vec<TaskInfo>, Vec<Node>) + Send + Sync + 'static,
    {
        self.on_deadlock = Some(Arc::new(f));
        self
    }

    pub fn start(self) -> Watchdog {
        let stall_threshold = self.stall_threshold_ms;
        let interval = self.sample_interval_ms;
        let on_stall = self.on_stall.unwrap_or_else(|| Arc::new(|_| {}));
        #[cfg(feature = "deadlock")]
        let on_deadlock = self.on_deadlock.unwrap_or_else(|| Arc::new(|_, _| {}));

        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(interval.max(10))).await;

                // --- stalled tasks ---
                let now = crate::monitor::now_millis();
                let snap = REGISTRY.snapshot();
                let stalled: Vec<TaskInfo> = snap
                    .into_iter()
                    .filter(|t| {
                        now.saturating_sub(
                            t.last_progress_ms
                                .load(std::sync::atomic::Ordering::Acquire),
                        ) > stall_threshold
                    })
                    .collect();
                if !stalled.is_empty() {
                    (on_stall)(stalled);
                }

                // --- deadlocks ---
                #[cfg(feature = "deadlock")]
                {
                    if let Some(cycle) = GRAPH.lock().detect_cycle() {
                        // collect task infos for each Task node
                        let tasks: Vec<_> = cycle
                            .iter()
                            .filter_map(|n| {
                                if let Node::Task(id) = n {
                                    REGISTRY.get(*id).map(|info| info)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        if !tasks.is_empty() {
                            on_deadlock(tasks, cycle);
                        }
                    }
                }
            }
        });

        Watchdog { handle }
    }
}

/// Start a deadlock watchdog that runs every `interval_ms` and calls `on_deadlock` with
/// (Vec<TaskInfo>, Vec<Node>) when a cycle is found. Returns JoinHandle for the watchdog.
pub fn init_deadlock_watchdog<F>(interval_ms: u64, mut on_deadlock: F) -> JoinHandle<()>
where
    F: FnMut(Vec<crate::monitor::TaskInfo>, Vec<Node>) + Send + 'static,
{
    tokio::spawn(async move {
        let interval = std::time::Duration::from_millis(interval_ms.max(10));
        loop {
            tokio::time::sleep(interval).await;
            if let Some(cycle) = GRAPH.lock().detect_cycle() {
                // collect task infos for each Task node
                let tasks: Vec<_> = cycle
                    .iter()
                    .filter_map(|n| {
                        if let Node::Task(id) = n {
                            REGISTRY.get(*id).map(|info| info)
                        } else {
                            None
                        }
                    })
                    .collect();
                if !tasks.is_empty() {
                    on_deadlock(tasks, cycle);
                }
            }
        }
    })
}
