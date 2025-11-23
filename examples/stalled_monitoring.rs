use detector::monitor::TaskInfo;
use detector::watchdog;
use tokio::time::{Duration, sleep};

#[detector_macros::monitored]
async fn looong_task() -> tokio::task::JoinHandle<usize> {
    let handle = tokio::spawn(async {
        tokio::task::yield_now().await;
        sleep(Duration::from_millis(500)).await;
        42usize
    });
    handle
}

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // Set up a callback to monitor stalled tasks
        let captured: std::sync::Arc<std::sync::Mutex<Vec<Vec<TaskInfo>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let cap_clone = captured.clone();

        let _w = watchdog::WatchdogBuilder::new()
            .stall_threshold_ms(200)
            .sample_interval_ms(50)
            .on_stall(move |stalled| {
                let mut guard = cap_clone.lock().unwrap();
                guard.push(stalled);
            })
            .start();

        let handle = looong_task().await;
        let res = handle.await.unwrap();
        println!("Result: {}", res);

        sleep(Duration::from_millis(300)).await;
        let captures = captured.lock().unwrap();
        let was_long_task_stalled = captures
            .iter()
            .flatten()
            .any(|t| t.location == Some("examples/stalled_monitoring.rs:5:1".to_string()));
        println!("Was looong task stalled? {}", was_long_task_stalled);
    });
}
