use detector::watchdog;
use detector::{GRAPH, MonitoredMutex, Node};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::time::{Duration, sleep};

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // Two mutexes to induce deadlock
        let m1 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m1"));
        let m2 = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(()), "m2"));

        let detected = Arc::new(StdMutex::new(false));
        let det_clone = detected.clone();

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

        #[detector_macros::monitored]
        async fn task_a(
            m1: Arc<MonitoredMutex<()>>,
            m2: Arc<MonitoredMutex<()>>,
        ) -> tokio::task::JoinHandle<()> {
            let handle = tokio::spawn(async move {
                let _g1 = m1.lock().await;
                sleep(Duration::from_millis(20)).await;
                let _g2 = m2.lock().await;
            });
            handle
        }

        #[detector_macros::monitored]
        async fn task_b(
            m1: Arc<MonitoredMutex<()>>,
            m2: Arc<MonitoredMutex<()>>,
        ) -> tokio::task::JoinHandle<()> {
            let handle = tokio::spawn(async move {
                let _g1 = m2.lock().await;
                sleep(Duration::from_millis(20)).await;
                let _g2 = m1.lock().await;
            });
            handle
        }

        let _h1 = task_a(m1.clone(), m2.clone()).await;
        let _h2 = task_b(m1.clone(), m2.clone()).await;

        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if *detected.lock().unwrap() {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("Deadlock not detected in time");

        GRAPH.lock().clear_edges_from(&Node::Lock("m1".to_string()));
        GRAPH.lock().clear_edges_from(&Node::Lock("m2".to_string()));
    });
}
