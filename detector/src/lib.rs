#[cfg(feature = "deadlock")]
pub mod graph;
pub mod monitor;
#[cfg(feature = "deadlock")]
pub mod monitored_mutex;
pub mod watchdog;

#[cfg(feature = "deadlock")]
pub use graph::{GRAPH, Node};
pub use monitor::{CURRENT_TASK_ID, REGISTRY, spawn_blocking_monitored, spawn_monitored};
#[cfg(feature = "deadlock")]
pub use monitored_mutex::MonitoredMutex;
#[cfg(feature = "deadlock")]
pub use watchdog::init_deadlock_watchdog;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};
    #[cfg(feature = "deadlock")]
    use tokio::time::timeout;
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn short_task_not_reported_as_stalled() {
        let captured: Arc<StdMutex<Vec<Vec<monitor::TaskInfo>>>> = Arc::new(StdMutex::new(vec![]));
        let c2 = captured.clone();

        // Start simple watchdog for stalls (not deadlock watcher) to ensure registry behavior is OK.
        let _w = watchdog::WatchdogBuilder::new()
            .stall_threshold_ms(200)
            .sample_interval_ms(50)
            .on_stall(move |stalled| {
                let mut g = c2.lock().unwrap();
                g.push(stalled);
            })
            .start();

        // spawn short task
        let h = spawn_monitored("short_task", async {
            tokio::task::yield_now().await;
            sleep(Duration::from_millis(30)).await;
            7usize
        });

        let r = h.await.unwrap();
        assert_eq!(r, 7usize);

        // give watchdog some time to run
        sleep(Duration::from_millis(300)).await;

        let g = captured.lock().unwrap();
        let was = g.iter().flatten().any(|t| t.name == "short_task");
        assert!(!was, "short task should not be reported as stalled");
    }

    #[tokio::test]
    async fn test_spawn_blocking_monitored_registers() {
        let handle = spawn_blocking_monitored("blocker", || {
            // simulate blocking work
            std::thread::sleep(std::time::Duration::from_millis(10));
            "ok"
        });

        let snap = REGISTRY.snapshot();

        let r = handle.await.unwrap();
        assert_eq!(r, "ok");

        // registry should have recorded a task with name "blocker"
        assert!(snap.iter().any(|t| t.name == "blocker"));
    }

    #[cfg(feature = "deadlock")]
    #[tokio::test]
    async fn detect_deadlock_two_mutexes() {
        // create two monitored mutexes
        let m1 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m1"));
        let m2 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m2"));

        // channel to notify when deadlock is detected
        let detected = Arc::new(StdMutex::new(false));
        let det_clone = detected.clone();

        // start deadlock watchdog (fast)
        let _hdl = init_deadlock_watchdog(50, move |tasks, cycles| {
            // record detection
            let mut d = det_clone.lock().unwrap();
            *d = true;
            for info in tasks {
                println!(
                    "Deadlock detected!\nTask: {}\nLocation: {:?}\nPolls: {}\nCycle nodes: {:?}\n",
                    info.name,
                    info.location,
                    info.polls.load(std::sync::atomic::Ordering::Acquire),
                    cycles
                );
            }
        });

        // spawn two tasks that will deadlock:
        // task A: lock m1, yield, then lock m2
        // task B: lock m2, yield, then lock m1
        let a_m1 = m1.clone();
        let a_m2 = m2.clone();
        let h1 = spawn_monitored("A", async move {
            let _g1 = a_m1.lock().await;
            // allow other task to acquire its first lock
            sleep(Duration::from_millis(20)).await;
            // this will block waiting for m2
            let _g2 = a_m2.lock().await;
        });

        let b_m1 = m1.clone();
        let b_m2 = m2.clone();
        let h2 = spawn_monitored("B", async move {
            let _g1 = b_m2.lock().await;
            sleep(Duration::from_millis(20)).await;
            let _g2 = b_m1.lock().await;
        });

        // wait up to 2 seconds for watchdog to detect deadlock
        let res = timeout(Duration::from_secs(2), async {
            loop {
                {
                    if *detected.lock().unwrap() {
                        break;
                    }
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await;

        assert!(res.is_ok(), "deadlock should be detected within timeout");

        // cleanup: abort tasks to not hang test forever (they are deadlocked)
        h1.abort();
        h2.abort();
    }
}
