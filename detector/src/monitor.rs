use std::{
    cell::Cell, future::Future, pin::Pin, sync::Arc, task::{Context, Poll}, time::{SystemTime, UNIX_EPOCH}
};
use parking_lot::RwLock;
use once_cell::sync::Lazy;
use pin_project_lite::pin_project;
use tokio::task::JoinHandle;
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    /// task-local TaskId so instrumented locks can find current task.
    /// Note: we set this in spawn_monitored when spawning the task.
    pub static CURRENT_TASK_ID: Cell<TaskId> = Cell::new(0);
}

/// Cheap monotonic-ish timestamp in milliseconds since program start (not wall-clock).
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_else(|_| 0)
}

/// Unique task id type.
pub type TaskId = u64;

/// Task information stored in registry.
#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub id: TaskId,
    pub name: String,
    pub polls: Arc<AtomicU64>,
    /// last time polled/completed/updated (ms)
    pub last_progress_ms: Arc<AtomicU64>,
    /// call-site location (file:line:col) captured with `#[track_caller]`
    pub location: Option<String>,
    /// whether it's a blocking/spawn_blocking task
    pub is_blocking: bool,
}

impl TaskInfo {
    fn touch(&self) {
        self.last_progress_ms
            .store(now_millis(), Ordering::Release);
    }
}

impl Default for TaskInfo {
    fn default() -> Self {
        Self {
            id: 0,
            name: "<unknown>".into(),
            polls: Arc::new(AtomicU64::new(0)),
            last_progress_ms: Arc::new(AtomicU64::new(now_millis())),
            location: None,
            is_blocking: false,
        }
    }
}

#[derive(Debug, Clone)]
struct RegistryInner {
    tasks: std::collections::HashMap<TaskId, TaskInfo>,
}

impl RegistryInner {
    fn new() -> Self {
        Self { tasks: Default::default() }
    }
}

/// Global registry for tasks.
/// RwLock chosen for many-read few-write pattern.
/// In hot paths we only touch atomics inside TaskInfo.
pub struct TaskRegistry {
    inner: Arc<RwLock<RegistryInner>>,
    next: Arc<AtomicU64>,
}

impl TaskRegistry {
    fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(RegistryInner::new())),
            next: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Register a TaskInfo (on spawn). Returns assigned TaskId.
    fn register(&self, mut info: TaskInfo) -> TaskId {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        info.id = id;
        info.touch();
        let mut guard = self.inner.write();
        guard.tasks.insert(id, info);
        id
    }

    /// Update last_progress timestamp for a task (cheap; uses atomic inside TaskInfo).
    fn touch(&self, id: TaskId) {
        if let Some(t) = self.inner.read().tasks.get(&id) {
            t.touch();
        }
    }

    /// Remove the task from the registry.
    fn deregister(&self, id: TaskId) {
        let mut guard = self.inner.write();
        guard.tasks.remove(&id);
    }

    /// Snapshot of current TaskInfo entries (cloned); for watchers and tests.
    pub fn snapshot(&self) -> Vec<TaskInfo> {
        self.inner
            .read()
            .tasks
            .values()
            .cloned()
            .collect::<Vec<_>>()
    }

    /// Find a TaskInfo by id.
    pub fn get(&self, id: TaskId) -> Option<TaskInfo> {
        self.inner.read().tasks.get(&id).cloned()
    }
}

pub static REGISTRY: Lazy<TaskRegistry> = Lazy::new(TaskRegistry::new);

pin_project! {
    /// MonitorFuture: wraps a future, increments poll count & updates last progress timestamp.
    pub struct MonitorFuture<F> {
        #[pin]
        inner: F,
        task_id: TaskId,
        info: TaskInfo,
    }
}

impl<F> MonitorFuture<F> {
    /// Construct with the task_id already assigned (no lazy registration).
    pub fn new(inner: F, task_id: TaskId, info: TaskInfo) -> Self {
        Self {
            inner,
            task_id,
            info,
        }
    }
}

impl<F: Future + Send + 'static> Future for MonitorFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Project the struct into pinned/unpinned parts.
        let this = self.project();

        // Register task if not yet registered
        if *this.task_id == 0 {
            let id = REGISTRY.register(this.info.clone());
            *this.task_id = id;
        }

        crate::monitor::CURRENT_TASK_ID.with( |id_cell| {
            id_cell.set(*this.task_id);
            // increment polls and touch timestamp
            this.info.polls.fetch_add(1, Ordering::Relaxed);
            this.info.touch();

            // delegate poll
            match this.inner.poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(output) => {
                    // Task just completed — mark progress and deregister to avoid later false positives.
                    let id = *this.task_id;
                    REGISTRY.touch(id);     // update last_progress to now
                    REGISTRY.deregister(id); // remove from registry so watchdog won't see it later
                    Poll::Ready(output)
                }
            }
        })
    }
}

/// Spawn a monitored async task.
///
/// `name`: human-friendly identification; `location` is captured from `#[track_caller]`.
#[track_caller]
pub fn spawn_monitored<F>(name: impl Into<String>, fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let loc = std::panic::Location::caller();
    let mut info = TaskInfo {
        id: 0,
        name: name.into(),
        polls: Arc::new(AtomicU64::new(0)),
        last_progress_ms: Arc::new(AtomicU64::new(now_millis())),
        location: Some(format!("{}:{}:{}", loc.file(), loc.line(), loc.column())),
        is_blocking: false,
    };

    // Register immediately and get an id
    let id = REGISTRY.register(info.clone());
    // ensure the info has id for MonitorFuture as well
    info.id = id;

    // Pass the id into MonitorFuture so we don't register lazily
    let mon = MonitorFuture::new(fut, id, info);
    tokio::spawn(mon)
}

/// Spawn a monitored blocking task (spawn_blocking).
///
/// For blocking tasks, we create a TaskInfo and register it immediately.
/// We update last_progress on registration and when task ends, then deregister.
#[track_caller]
pub fn spawn_blocking_monitored<F, R>(name: impl Into<String>, f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let loc = std::panic::Location::caller();
    let info = TaskInfo {
        id: 0,
        name: name.into(),
        polls: Arc::new(AtomicU64::new(0)),
        last_progress_ms: Arc::new(AtomicU64::new(now_millis())),
        location: Some(format!("{}:{}:{}", loc.file(), loc.line(), loc.column())),
        is_blocking: true,
    };

    // Register immediately before spawn_blocking to ensure we know the task exists.
    let id = REGISTRY.register(info.clone());
    // spawn_blocking closure will touch timestamp on start, and deregister on complete.
    tokio::task::spawn_blocking(move || {
        // touch at start
        REGISTRY.touch(id);
        let res = f();
        println!("Task was complete");
        // touch on complete
        REGISTRY.touch(id);
        // then remove from registry to avoid false alarms
        REGISTRY.deregister(id);
        res
    })
}

/// Initialize watchdog background task; returns the JoinHandle so caller can keep/stop it.
/// `stall_ms` — threshold in milliseconds to consider a task stalled.
/// `sample_interval_ms` — how often to check.
pub fn init_watchdog<F>(stall_ms: u64, sample_interval_ms: u64, mut on_stall: F) -> JoinHandle<()>
where
    F: FnMut(Vec<TaskInfo>) + Send + 'static,
{
    tokio::spawn(async move {
        let interval = tokio::time::Duration::from_millis(sample_interval_ms.max(10));
        loop {
            tokio::time::sleep(interval).await;
            let snap = REGISTRY.snapshot();
            let now = now_millis();
            let stalled: Vec<TaskInfo> = snap
                .into_iter()
                .filter(|t| now.saturating_sub(t.last_progress_ms.load(Ordering::Acquire)) > stall_ms)
                .collect();
            if !stalled.is_empty() {
                on_stall(stalled);
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    /// Reset the registry before each test.
    fn clear_registry() {
        REGISTRY.inner.write().tasks.clear();
        REGISTRY.next.store(1, Ordering::Relaxed);
    }

    #[tokio::test]
    async fn test_registry_register_and_snapshot() {
        clear_registry();

        let info = TaskInfo::default();
        let id = REGISTRY.register(info.clone());

        let snap = REGISTRY.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, id);
    }

    #[tokio::test]
    async fn test_registry_touch_updates_timestamp() {
        clear_registry();

        let info = TaskInfo::default();
        let id = REGISTRY.register(info.clone());
        let before = REGISTRY.get(id).unwrap().last_progress_ms.load(Ordering::Acquire);

        tokio::time::sleep(Duration::from_millis(5)).await;
        REGISTRY.touch(id);

        let after = REGISTRY.get(id).unwrap().last_progress_ms.load(Ordering::Acquire);
        assert!(after > before);
    }

    #[tokio::test]
    async fn test_registry_deregister() {
        clear_registry();

        let id = REGISTRY.register(TaskInfo::default());
        assert!(REGISTRY.get(id).is_some());

        REGISTRY.deregister(id);
        assert!(REGISTRY.get(id).is_none());
    }

    // ---- MonitorFuture -------------------------------------------------------

    #[tokio::test]
    async fn test_monitorfuture_counts_polls_and_deregisters() {
        clear_registry();

        let fut = async {
            tokio::task::yield_now().await;
            123
        };

        let info = TaskInfo::default();
        let mon = MonitorFuture::new(fut, 0, info.clone());
        let handle = tokio::spawn(mon);

        let output = handle.await.unwrap();
        assert_eq!(output, 123);

        // After completion, task must be removed.
        assert!(REGISTRY.snapshot().is_empty());
    }

    // ---- spawn_monitored -----------------------------------------------------

    #[tokio::test]
    async fn test_spawn_monitored_registers_and_deregisters() {
        clear_registry();

        let handle = spawn_monitored("test-task", async {
            tokio::task::yield_now().await;
            42
        });

        // Immediately after spawn, task should be in registry
        let snap = REGISTRY.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].id, 1);

        assert_eq!(snap[0].name, "test-task");
        assert!(!snap[0].is_blocking);

        let result = handle.await.unwrap();
        assert_eq!(result, 42);

        // It should be removed after completion
        assert!(REGISTRY.snapshot().is_empty());
    }

    // ---- spawn_blocking_monitored -------------------------------------------

    #[tokio::test]
    async fn test_spawn_blocking_monitored() {
        clear_registry();

        let handle = spawn_blocking_monitored("block", || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            777
        });

        // Immediately after spawn, it should exist
        let snap = REGISTRY.snapshot();
        assert_eq!(snap.len(), 1);
        assert!(snap[0].is_blocking);

        let value = handle.await.unwrap();
        assert_eq!(value, 777);

        // After completing, it should be removed
        assert!(REGISTRY.snapshot().is_empty());
    }

    // ---- init_watchdog -------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_watchdog_detects_stalled_tasks() {
        clear_registry();

        // Register a task with an artificially old timestamp
        let info = TaskInfo {
            id: 0,
            name: "stall-me".into(),
            polls: Arc::new(AtomicU64::new(0)),
            last_progress_ms: Arc::new(AtomicU64::new(now_millis() - 10_000)), // very old
            location: None,
            is_blocking: false,
        };

        let id = REGISTRY.register(info.clone());

        let triggered = Arc::new(AtomicU64::new(0));
        let triggered_c = triggered.clone();

        // stall_ms = 50 ms, interval = 10 ms
        let _watchdog = init_watchdog(50, 10, move |stalled| {
            assert_eq!(stalled.len(), 1);
            assert_eq!(stalled[0].id, id);
            triggered_c.fetch_add(1, Ordering::Relaxed);
        });

        sleep(Duration::from_millis(60)).await;

        assert!(
            triggered.load(Ordering::Relaxed) > 0,
            "watchdog should have flagged the stalled task"
        );
    }
}
