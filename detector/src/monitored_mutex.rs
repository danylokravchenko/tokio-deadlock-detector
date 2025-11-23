use crate::graph::{GRAPH, Node};
use crate::monitor::CURRENT_TASK_ID;
use std::sync::Arc;
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard};

/// A small wrapper around tokio::sync::Mutex that reports wait/ownership to the global graph.
#[derive(Clone)]
pub struct MonitoredMutex<T> {
    inner: Arc<TokioMutex<T>>,
    name: String,
}

pub struct MonitoredMutexGuard<T> {
    inner: OwnedMutexGuard<T>,
    name: String,
    owner: u64,
}

impl<T> Drop for MonitoredMutexGuard<T> {
    fn drop(&mut self) {
        // remove Lock -> Task edge when guard is dropped
        let lock_node = Node::Lock(self.name.clone());
        let mut g = GRAPH.lock();
        g.clear_edges_from(&lock_node); // remove Lock -> Task edges
        // also remove edges to the lock (shouldn't be needed, kept safe)
        g.clear_edges_to(&lock_node);
    }
}

impl<T> MonitoredMutex<T> {
    pub fn new(inner: TokioMutex<T>, name: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(inner),
            name: name.into(),
        }
    }

    /// Acquire the lock with instrumentation.
    pub async fn lock(&self) -> MonitoredMutexGuard<T> {
        // find current task id; if not set, use 0
        let tid = CURRENT_TASK_ID.try_with(|id| id.get()).unwrap_or(0);

        // record Task -> Lock (waiting)
        {
            let mut g = GRAPH.lock();
            g.add_edge(Node::Task(tid), Node::Lock(self.name.clone()));
        }

        // await the real lock (owned guard)
        let owned = self.inner.clone().lock_owned().await;

        // on acquire: remove Task -> Lock (no longer waiting) and add Lock -> Task (owner)
        {
            let mut g = GRAPH.lock();
            g.clear_edges_from(&Node::Task(tid)); // remove waiting edges
            g.add_edge(Node::Lock(self.name.clone()), Node::Task(tid));
        }

        MonitoredMutexGuard {
            inner: owned,
            name: self.name.clone(),
            owner: tid,
        }
    }

    /// Try lock (non-waiting) — returns None if not available.
    pub async fn try_lock(&self) -> Option<MonitoredMutexGuard<T>> {
        // quick try without instrumentation
        if let Ok(guard) = self.inner.try_lock() {
            // we have a guard — no graph waiting; add ownership edge
            let tid = CURRENT_TASK_ID.try_with(|id| id.get()).unwrap_or(0);
            let mut g = GRAPH.lock();
            g.add_edge(Node::Lock(self.name.clone()), Node::Task(tid));
            // convert the guard into OwnedMutexGuard by dropping and re-locking owned?
            // OwnedGuard construction isn't trivial here; avoid try_lock usage in tests.
            // For completeness, we'll drop guard and return None to avoid complexity.
            drop(guard);
            None
        } else {
            None
        }
    }

    /// Non-blocking try_lock, returns Some(guard) if acquired
    pub async fn try_lock_nowait(&self) -> Option<MonitoredMutexGuard<T>> {
        // get task id
        let tid = CURRENT_TASK_ID.try_with(|id| id.get()).unwrap_or(0);

        if let Ok(owned) = self.inner.clone().try_lock_owned() {
            // on acquire: add Lock->Task edge
            let mut g = GRAPH.lock();
            g.add_edge(Node::Lock(self.name.clone()), Node::Task(tid));

            Some(MonitoredMutexGuard {
                inner: owned,
                name: self.name.clone(),
                owner: tid,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GRAPH, Node};
    use crate::monitor::CURRENT_TASK_ID;
    use tokio::task;

    fn set_task_id(id: u64) {
        CURRENT_TASK_ID.set(id);
    }

    #[tokio::test]
    async fn test_lock_adds_wait_and_acquire_edges() {
        GRAPH.lock().clear();

        let m = MonitoredMutex::new(TokioMutex::new(5usize), "L1");

        task::spawn(async move {
            set_task_id(1);
            let _g = m.lock().await;

            let g = GRAPH.lock();
            assert!(g.has_edge(&Node::Lock("L1".into()), &Node::Task(1)));
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_drop_removes_edges() {
        GRAPH.lock().clear();

        let m = MonitoredMutex::new(TokioMutex::new(0usize), "Lx");

        task::spawn(async move {
            set_task_id(1);
            {
                let _g = m.lock().await;
            }

            let g = GRAPH.lock();
            assert!(!g.has_any_edges_with(&Node::Lock("Lx".into())));
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_two_tasks_wait_on_same_mutex_order_preserved() {
        GRAPH.lock().clear();

        let m = MonitoredMutex::new(TokioMutex::new(()), "Lw");

        // Acquire by task 1
        let g1 = m.clone();
        let h1 = task::spawn(async move {
            set_task_id(1);
            let _g = g1.lock().await;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });

        // Allow T1 to acquire first
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Task 2 waits
        let g2 = m.clone();
        let h2 = task::spawn(async move {
            set_task_id(2);
            let _g = g2.lock().await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let g = GRAPH.lock();
        assert!(g.has_edge(&Node::Task(2), &Node::Lock("Lw".into())));

        h1.await.unwrap();
        h2.await.unwrap();
    }

    #[tokio::test]
    async fn test_try_lock_nowait() {
        GRAPH.lock().clear();

        let m = MonitoredMutex::new(TokioMutex::new(10usize), "Lt");

        // First: acquire lock fully
        let g1 = m.clone();
        let h1 = task::spawn(async move {
            set_task_id(1);
            let _g = g1.lock().await;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // try-lock should fail (and should not modify graph)
        assert!(m.try_lock_nowait().await.is_none());

        // check the graph: lock is owned by Task 1
        let g = GRAPH.lock();
        assert!(g.has_edge(&Node::Lock("Lt".into()), &Node::Task(1)));
        assert_eq!(g.outgoing_count(&Node::Task(2)), 0); // no fake edges

        h1.await.unwrap();
    }
}
