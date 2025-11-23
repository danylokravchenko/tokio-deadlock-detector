use crate::{monitor::REGISTRY, graph::{GRAPH, Node}};
use tokio::task::JoinHandle;

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
                let tasks: Vec<_> = cycle.iter().filter_map(|n| {
                    if let Node::Task(id) = n {
                        REGISTRY.get(*id).map(|info| info)
                    } else { None }
                }).collect();
                if !tasks.is_empty() {
                    on_deadlock(tasks, cycle);
                }
            }
        }
    })
}
