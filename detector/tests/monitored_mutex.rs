// tests/monitored_mutex.rs
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use detector::{MonitoredMutex, spawn_monitored};

/// Test that MonitoredMutex enforces mutual exclusion and allows serialized access.
/// Task A acquires the lock first, writes `1` and holds it for 100ms.
/// Task B starts a little later, blocks on the lock, and when it acquires it it should observe `1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_mutex_mutation_and_exclusion() {
    let m = Arc::new(MonitoredMutex::new(tokio::sync::Mutex::new(0), "m1"));

    // channel to send B's observed value back to the test thread
    let (tx, rx) = tokio::sync::oneshot::channel::<i32>();

    // Task A: hold lock, set value to 1, sleep, then release
    let a_m = m.clone();
    let h_a = spawn_monitored("A", async move {
        let mut g = a_m.lock().await;
        *g = 1;
        // verify guard deref works: should be able to mutate inner value
        assert_eq!(*g, 1);
        // keep lock for 100 ms
        sleep(Duration::from_millis(100)).await;
        // guard dropped at end of scope
    });

    // Task B: start after a short delay, then try to acquire and read the value
    let b_m = m.clone();
    let h_b = spawn_monitored("B", async move {
        // start slightly after A to ensure A acquires first
        sleep(Duration::from_millis(10)).await;
        let g = b_m.lock().await;
        let seen = *g;
        // send seen value back
        let _ = tx.send(seen);
        // guard dropped at end of scope
    });

    // Wait for both tasks to finish
    let _ = h_a.await;
    let _ = h_b.await;

    // Get observed value from B
    let seen_by_b = rx.await.expect("B should send its observed value");
    assert_eq!(
        seen_by_b, 1,
        "B should have observed A's update (mutual exclusion ensured)"
    );
}

/// Test that MonitoredMutexGuard reports name() and owner() and that name is the same as the mutex's name.
/// We spawn a monitored task so CURRENT_TASK_ID is set, then inspect the guard and send the info back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_guard_owner_and_name() {
    let m = Arc::new(MonitoredMutex::new(
        tokio::sync::Mutex::new(0usize),
        "my_lock",
    ));

    // channel to receive (owner, name)
    let (tx, rx) = tokio::sync::oneshot::channel::<(u64, String)>();

    let m2 = m.clone();
    let _h = spawn_monitored("inspect_guard", async move {
        let g = m2.lock().await;
        let owner = g.owner();
        let name = g.name().to_string();

        // Basic checks inside the task (sanity)
        assert_eq!(name, "my_lock");
        assert!(
            owner != 0,
            "owner should be nonzero for a spawned monitored task"
        );

        // Send values back
        let _ = tx.send((owner, name));
        // guard dropped at end of scope
    });

    let (owner, name) = rx.await.expect("should receive guard info");
    assert_eq!(name, "my_lock");
    assert!(owner != 0, "owner should be nonzero");
}
