# tokio-deadlock-detector

Detect stalls and deadlocks in the tokio async runtime.

## Features

### Stalled Tasks Detection

- Monitors all spawned async tasks in a tokio runtime.
- Tracks task progress and polls, reporting tasks that have not made progress for a configurable threshold.
- Provides a `Watchdog` API to register callbacks for stalled tasks.
- Integrates with procedural macro `#[monitored]` to automatically rewrite `tokio::spawn` and `tokio::task::spawn_blocking` calls to monitored versions.

### Deadlock Detection

- Monitors lock acquisition and release for `MonitoredMutex` (use this type for lock tracking).
- Detects cycles in the lock/task graph, reporting deadlocks and the involved tasks.
- Provides a `Watchdog` API to register callbacks for deadlock events.
- Integrates with procedural macro `#[monitored]` to track lock ownership and task relationships.

## Usage

### Stalled Tasks Example

```rust
use detector_macros::monitored;

#[monitored]
async fn my_task() -> String {
    tokio::spawn(async {
        tokio::time::sleepsleep(std::time::Duration::from_secs(1)).await;
        "ok".to_string()
    }).await
}

fn main() {
    let _w = watchdog::WatchdogBuilder::new()
        .stall_threshold_ms(200)
        .sample_interval_ms(50)
        .on_stall(move |stalled| {
            // task was stalled
        })
        .start();
    my_task().await;
}
```

### Deadlock Detection Example

```rust
use detector::MonitoredMutex;
use detector_macros::monitored;
use std::sync::Arc;

#[monitored]
async fn lock_tasks(m1: Arc<MonitoredMutex<()>>, m2: Arc<MonitoredMutex<()>>) {
    let m1_clone = m1.clone();
    let m2_clone = m2.clone();
    let handle1 = tokio::spawn(async move {
        let _g1 = m1_clone.lock().await;
        sleep(Duration::from_millis(20)).await;
        let _g2 = m2_clone.lock().await;
    });
    let handle2 = tokio::spawn(async move {
        let _g1 = m2.lock().await;
        sleep(Duration::from_millis(20)).await;
        let _g2 = m1.lock().await;
    });
    handle1.await;
    handle2.await;
}

fn main() {
    let _w = watchdog::WatchdogBuilder::new()
        .sample_interval_ms(50)
        .on_deadlock(move |tasks, cycle| {
            for info in tasks {
                println!(
                    "Deadlock detected! Task: {}, Location: {:?}, Polls: {}, Cycle: {:?}",
                    info.name,
                    info.location,
                    info.polls.load(std::sync::atomic::Ordering::Acquire),
                    cycle
                );
            }
        })
        .start();
    let m1 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m1"));
    let m2 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m2"));
    lock_tasks(m1, m2).await;
}
```

## Limitations

- **Procedural macro limitations:**
  - The macro cannot rewrite function signatures; you must update return/argument types manually if you use `MonitoredMutex`.
  - Aliased imports are only detected inside the function body, not at module scope.
- **Task naming:**
  - By default, the macro uses the call name or function name for task identification.
- **Single runtime:**
  - The monitoring is designed for a single tokio runtime per process. Custom tokio-based threadpools are not captured by this crate.
- **Performance:**
  - Monitoring and graph tracking add some overhead, but are optimized for production use.

## Getting Started

1. Add `detector` and `detector-macros` to your dependencies.
2. Use the `#[monitored]` macro on async functions or blocks that spawn tasks or use mutexes.
3. Set up a `Watchdog` to monitor stalled tasks and deadlocks.
4. Enable the `deadlock` feature if you want deadlock detection:

   ```sh
   cargo run --example deadlock --features deadlock
   ```

## License

Apache 2.0
