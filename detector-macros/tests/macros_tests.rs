#[cfg(feature = "deadlock")]
use std::sync::{Arc, Mutex as StdMutex};

use detector::monitor::{REGISTRY, TaskInfo, init_watchdog, spawn_monitored};
#[cfg(feature = "deadlock")]
use detector::{GRAPH, MonitoredMutex, Node, init_deadlock_watchdog};
use detector_macros::monitored;
use tokio::{
    task::JoinHandle,
    time::{Duration, sleep},
};

#[tokio::test]
async fn test_spawn_monitored_progress_and_watchdog() {
    // callback collects stalled tasks
    let captured: std::sync::Arc<std::sync::Mutex<Vec<Vec<TaskInfo>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let cap_clone = captured.clone();

    // start watchdog: stall threshold 200 ms, sample every 50ms
    let _hdl = init_watchdog(200, 50, move |stalled| {
        let mut guard = cap_clone.lock().unwrap();
        guard.push(stalled);
    });

    let handle = spawn_monitored("short", async {
        // yield a couple times and finish quickly
        tokio::task::yield_now().await;
        sleep(Duration::from_millis(30)).await;
        42usize
    });

    let res = handle.await.unwrap();
    assert_eq!(res, 42usize);

    // sleep to let watchdog run — short task shouldn't be reported as stalled
    sleep(Duration::from_millis(300)).await;

    let captures = captured.lock().unwrap();
    // there might be other tasks (watchdog tasks), but there should be at least one snapshot run.
    // assert that short task was not captured as stalled (its name not present in any snapshot)
    let was_short_stalled = captures.iter().flatten().any(|t| t.name == "short");
    assert!(
        !was_short_stalled,
        "short task should not be reported as stalled"
    );
}

#[tokio::test]
async fn test_spawn_blocking_monitored_registers() {
    #[monitored]
    fn task_blocker() -> JoinHandle<String> {
        let handle = tokio::task::spawn_blocking(|| {
            std::thread::sleep(Duration::from_millis(20));
            "ok".to_string()
        });
        handle
    }
    let handle = task_blocker();
    let snap = REGISTRY.snapshot();
    println!("Snapshot: {:?}", snap);

    let r = handle.await.unwrap();
    assert_eq!(r, "ok");

    // registry should have recorded a task
    assert_eq!(snap.len(), 1);
}

#[tokio::test]
async fn test_spawn_monitored_registers() {
    #[monitored]
    async fn task_blocker() -> JoinHandle<String> {
        let handle = tokio::spawn(async {
            sleep(Duration::from_millis(20)).await;
            "ok".to_string()
        });
        handle
    }
    let handle = task_blocker().await;
    let snap = REGISTRY.snapshot();
    println!("Snapshot: {:?}", snap);

    let r = handle.await.unwrap();
    assert_eq!(r, "ok");

    // registry should have recorded a task
    assert_eq!(snap.len(), 1);
}

#[cfg(feature = "deadlock")]
#[tokio::test]
async fn test_macro_and_monitored_mutex_integration() {
    // Two mutexes to induce deadlock
    let m1 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m1"));
    let m2 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m2"));

    // Channel to capture deadlock detection
    let detected = Arc::new(StdMutex::new(false));
    let det_clone = detected.clone();

    // Start deadlock watchdog
    let _hdl = init_deadlock_watchdog(50, move |tasks, cycle| {
        for info in tasks {
            println!(
                "Deadlock detected! Task: {}, Location: {:?}, Polls: {}, Cycle: {:?}",
                info.name,
                info.location,
                info.polls.load(std::sync::atomic::Ordering::Acquire),
                cycle
            );
        }
        let mut d = det_clone.lock().unwrap();
        *d = true;
    });

    // Macro-generated tasks using your procedural macro
    #[monitored]
    async fn task_a(m1: Arc<MonitoredMutex<()>>, m2: Arc<MonitoredMutex<()>>) {
        let _g1 = m1.lock().await;
        sleep(Duration::from_millis(20)).await;
        let _g2 = m2.lock().await;
    }

    #[monitored]
    async fn task_b(m1: Arc<MonitoredMutex<()>>, m2: Arc<MonitoredMutex<()>>) {
        let _g1 = m2.lock().await;
        sleep(Duration::from_millis(20)).await;
        let _g2 = m1.lock().await;
    }

    // Spawn macro-generated tasks
    let _h1 = task_a(m1.clone(), m2.clone());
    let _h2 = task_b(m1.clone(), m2.clone());

    // Wait for deadlock detection (up to 2 seconds)
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if *detected.lock().unwrap() {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Deadlock not detected in time");

    // Abort tasks to clean up
    GRAPH.lock().clear_edges_from(&Node::Lock("m1".to_string()));
    GRAPH.lock().clear_edges_from(&Node::Lock("m2".to_string()));
}
