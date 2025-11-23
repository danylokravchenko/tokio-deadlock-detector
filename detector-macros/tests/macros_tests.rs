#[cfg(feature = "deadlock")]
use std::sync::{Arc, Mutex as StdMutex};

#[cfg(feature = "deadlock")]
use detector::{GRAPH, MonitoredMutex, Node};
use detector::{
    monitor::{REGISTRY, TaskInfo, spawn_monitored},
    watchdog,
};
use detector_macros::monitored;
use serial_test::serial;
use tokio::{
    task::JoinHandle,
    time::{Duration, sleep},
};

#[tokio::test]
#[serial]
async fn test_spawn_monitored_progress_and_watchdog() {
    // callback collects stalled tasks
    let captured: std::sync::Arc<std::sync::Mutex<Vec<Vec<TaskInfo>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let cap_clone = captured.clone();

    // start watchdog: stall threshold 200 ms, sample every 50ms
    let _w = watchdog::WatchdogBuilder::new()
        .stall_threshold_ms(200)
        .sample_interval_ms(50)
        .on_stall(move |stalled| {
            let mut guard = cap_clone.lock().unwrap();
            guard.push(stalled);
        })
        .start();

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
#[serial]
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
#[serial]
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
#[serial]
async fn test_macro_and_monitored_mutex_integration() {
    // Two mutexes to induce deadlock
    let m1 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m1"));
    let m2 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m2"));

    // Channel to capture deadlock detection
    let detected = Arc::new(StdMutex::new(false));
    let det_clone = detected.clone();

    // Start deadlock watchdog
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
            let mut d = det_clone.lock().unwrap();
            *d = true;
        })
        .start();

    // Macro-generated tasks using your procedural macro
    #[monitored]
    async fn task_a(m1: Arc<MonitoredMutex<()>>, m2: Arc<MonitoredMutex<()>>) -> JoinHandle<()> {
        let handle = tokio::spawn(async move {
            let _g1 = m1.lock().await;
            sleep(Duration::from_millis(20)).await;
            let _g2 = m2.lock().await;
        });
        handle
    }

    #[monitored]
    async fn task_b(m1: Arc<MonitoredMutex<()>>, m2: Arc<MonitoredMutex<()>>) -> JoinHandle<()> {
        let handle = tokio::spawn(async move {
            let _g1 = m2.lock().await;
            sleep(Duration::from_millis(20)).await;
            let _g2 = m1.lock().await;
        });
        handle
    }

    // Spawn macro-generated tasks
    let _h1 = task_a(m1.clone(), m2.clone()).await;
    let _h2 = task_b(m1.clone(), m2.clone()).await;

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

#[tokio::test]
#[serial]
async fn test_macro_rewrites_spawn_import() {
    use detector::monitor::REGISTRY;
    #[allow(unused)]
    use tokio::spawn;
    #[monitored]
    async fn spawn_task() -> tokio::task::JoinHandle<&'static str> {
        let handle = spawn(async { "rewritten_spawn" });
        handle
    }
    let handle = spawn_task().await;
    let snap = REGISTRY.snapshot();
    let r = handle.await.unwrap();
    assert_eq!(r, "rewritten_spawn");
    assert_eq!(snap.len(), 1);
}

#[tokio::test]
#[serial]
async fn test_macro_rewrites_spawn_blocking_alias() {
    use detector::monitor::REGISTRY;
    #[monitored]
    fn spawn_blocking_task() -> tokio::task::JoinHandle<&'static str> {
        #[allow(unused)]
        use tokio::task::spawn_blocking as spawn_b;
        let handle = spawn_b(|| "rewritten_blocking");
        handle
    }
    let handle = spawn_blocking_task();
    let snap = REGISTRY.snapshot();
    let r = handle.await.unwrap();
    assert_eq!(r, "rewritten_blocking");
    assert_eq!(snap.len(), 1);
}

#[tokio::test]
#[serial]
async fn test_macro_rewrites_explicit_paths() {
    use detector::monitor::REGISTRY;
    #[monitored]
    async fn explicit_spawn_task() -> tokio::task::JoinHandle<&'static str> {
        let handle = tokio::spawn(async { "explicit_path" });
        handle
    }
    #[monitored]
    fn explicit_spawn_blocking_task() -> tokio::task::JoinHandle<&'static str> {
        let handle = tokio::task::spawn_blocking(|| "explicit_blocking");
        handle
    }
    let handle1 = explicit_spawn_task().await;
    let handle2 = explicit_spawn_blocking_task();
    let snap = REGISTRY.snapshot();
    let r1 = handle1.await.unwrap();
    assert_eq!(r1, "explicit_path");
    let r2 = handle2.await.unwrap();
    assert_eq!(r2, "explicit_blocking");
    assert!(snap.len() >= 2);
}
