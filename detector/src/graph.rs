use std::collections::{HashMap, HashSet};

pub type TaskId = u64;

/// Locks are uniquely identified by string names or internal IDs.
/// You can substitute with usize, u64, etc.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Node {
    Task(TaskId),
    Lock(String),
}

/// A Wait-For graph used to detect cycles.
#[derive(Debug, Default)]
pub struct WaitForGraph {
    /// adjacency list: Node → set of neighbors that this node waits on
    edges: HashMap<Node, HashSet<Node>>,
}

impl WaitForGraph {
    pub fn new() -> Self {
        Self {
            edges: HashMap::new(),
        }
    }

    /// Clear the entire graph.
    pub fn clear(&mut self) {
        self.edges.clear();
    }

    /// Add or replace directed edge A → B (A waits on B)
    pub fn add_edge(&mut self, from: Node, to: Node) {
        self.edges.entry(from).or_default().insert(to);
    }

    /// Returns true if there is an edge `from → to`.
    pub fn has_edge(&self, from: &Node, to: &Node) -> bool {
        self.edges
            .get(from)
            .map(|set| set.contains(to))
            .unwrap_or(false)
    }

    /// Returns true if *any* edge touches this node (either incoming or outgoing).
    pub fn has_any_edges_with(&self, n: &Node) -> bool {
        // outgoing edges
        if self.edges.get(n).map(|s| !s.is_empty()).unwrap_or(false) {
            return true;
        }
        // incoming edges
        self.edges.values().any(|set| set.contains(n))
    }

    /// Returns the number of outgoing edges.
    pub fn outgoing_count(&self, from: &Node) -> usize {
        self.edges.get(from).map(|s| s.len()).unwrap_or(0)
    }

    /// Return all outgoing neighbors (clone).
    pub fn outgoing(&self, from: &Node) -> Vec<Node> {
        self.edges
            .get(from)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Remove all edges originating from a node (task finished or lock released)
    pub fn clear_edges_from(&mut self, n: &Node) {
        self.edges.remove(n);
    }

    /// Remove all edges into a node (lock released)
    pub fn clear_edges_to(&mut self, n: &Node) {
        for (_, set) in self.edges.iter_mut() {
            set.remove(n);
        }
    }

    /// Remove all nodes for a task (task ended)
    pub fn remove_task(&mut self, id: TaskId) {
        let t = Node::Task(id);
        self.clear_edges_from(&t);
        self.clear_edges_to(&t);
    }

    /// Cycle detection using DFS.
    pub fn detect_cycle(&self) -> Option<Vec<Node>> {
        #[derive(Clone, Copy)]
        enum Color {
            White,
            Gray,
            Black,
        }

        let mut color: HashMap<&Node, Color> = HashMap::new();
        let mut parent: HashMap<&Node, &Node> = HashMap::new();

        for n in self.edges.keys() {
            color.insert(n, Color::White);
        }

        fn dfs<'a>(
            graph: &'a HashMap<Node, HashSet<Node>>,
            u: &'a Node,
            color: &mut HashMap<&'a Node, Color>,
            parent: &mut HashMap<&'a Node, &'a Node>,
        ) -> Option<Vec<Node>> {
            color.insert(u, Color::Gray);

            if let Some(children) = graph.get(u) {
                for v in children {
                    match color.get(v).copied().unwrap_or(Color::White) {
                        Color::White => {
                            parent.insert(v, u);
                            if let Some(cycle) = dfs(graph, v, color, parent) {
                                return Some(cycle);
                            }
                        }
                        Color::Gray => {
                            // Found a back-edge → cycle
                            let mut cycle = vec![v.clone()];
                            let mut cur = u;
                            while cur != v {
                                cycle.push(cur.clone());
                                cur = parent[cur];
                            }
                            cycle.reverse();
                            return Some(cycle);
                        }
                        Color::Black => {}
                    }
                }
            }

            color.insert(u, Color::Black);
            None
        }

        for n in self.edges.keys() {
            if let Color::White = color[n] {
                if let Some(cycle) = dfs(&self.edges, n, &mut color, &mut parent) {
                    return Some(cycle);
                }
            }
        }

        None
    }

    /// Return all nodes currently present in the graph.
    pub fn nodes(&self) -> Vec<Node> {
        let mut out = Vec::new();

        // outgoing nodes
        for k in self.edges.keys() {
            out.push(k.clone());
        }

        // nodes only appearing as destinations
        for set in self.edges.values() {
            for v in set {
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
        }

        out
    }
}

/// Global graph
use once_cell::sync::Lazy;
use parking_lot::Mutex;

pub static GRAPH: Lazy<Mutex<WaitForGraph>> = Lazy::new(|| Mutex::new(WaitForGraph::new()));

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64) -> Node {
        Node::Task(id)
    }
    fn l(s: &str) -> Node {
        Node::Lock(s.into())
    }

    #[test]
    fn test_add_and_has_edge() {
        let mut g = WaitForGraph::new();
        let a = Node::Task(1);
        let b = Node::Lock("L1".into());

        assert!(!g.has_edge(&a, &b));

        g.add_edge(a.clone(), b.clone());

        assert!(g.has_edge(&a, &b));
        assert!(!g.has_edge(&b, &a));
    }

    #[test]
    fn test_clear_edges_from() {
        let mut g = WaitForGraph::new();
        let a = Node::Task(1);
        let b = Node::Lock("L1".into());
        let c = Node::Lock("L2".into());

        g.add_edge(a.clone(), b.clone());
        g.add_edge(a.clone(), c.clone());

        assert!(g.has_edge(&a, &b));
        assert!(g.has_edge(&a, &c));

        g.clear_edges_from(&a);

        assert!(!g.has_edge(&a, &b));
        assert!(!g.has_edge(&a, &c));
    }

    #[test]
    fn test_clear_edges_to() {
        let mut g = WaitForGraph::new();
        let a = Node::Task(1);
        let b = Node::Task(2);

        let lock = Node::Lock("L".into());

        g.add_edge(a.clone(), lock.clone());
        g.add_edge(b.clone(), lock.clone());
        assert!(g.has_edge(&a, &lock));
        assert!(g.has_edge(&b, &lock));

        g.clear_edges_to(&lock);

        assert!(!g.has_edge(&a, &lock));
        assert!(!g.has_edge(&b, &lock));
    }

    #[test]
    fn test_remove_task() {
        let mut g = WaitForGraph::new();
        let t1 = Node::Task(1);
        let t2 = Node::Task(2);

        g.add_edge(t1.clone(), t2.clone());
        g.add_edge(t2.clone(), t1.clone());

        g.remove_task(1);

        // all edges from and to t1 must be gone
        assert!(!g.has_edge(&t1, &t2));
        assert!(!g.has_edge(&t2, &t1));

        // but task 2 may still exist as a key (empty or missing is fine)
        assert!(!g.has_any_edges_with(&t1));
    }

    #[test]
    fn test_has_any_edges_with() {
        let mut g = WaitForGraph::new();
        let t1 = Node::Task(1);
        let t2 = Node::Task(2);
        let l = Node::Lock("L".into());

        assert!(!g.has_any_edges_with(&t1));

        g.add_edge(t1.clone(), t2.clone());
        assert!(g.has_any_edges_with(&t1));
        assert!(g.has_any_edges_with(&t2));

        g.add_edge(l.clone(), t1.clone());
        assert!(g.has_any_edges_with(&l));
    }

    #[test]
    fn test_outgoing_and_count() {
        let mut g = WaitForGraph::new();
        let a = Node::Task(1);
        let b = Node::Lock("B".into());
        let c = Node::Lock("C".into());

        assert_eq!(g.outgoing_count(&a), 0);
        assert_eq!(g.outgoing(&a).len(), 0);

        g.add_edge(a.clone(), b.clone());
        g.add_edge(a.clone(), c.clone());

        let out = g.outgoing(&a);
        assert_eq!(out.len(), 2);

        // order is not guaranteed; convert to set
        let out_set: HashSet<_> = out.into_iter().collect();
        assert!(out_set.contains(&b));
        assert!(out_set.contains(&c));
    }

    #[test]
    fn test_clear_graph() {
        let mut g = WaitForGraph::new();

        let a = Node::Task(1);
        let b = Node::Task(2);

        g.add_edge(a.clone(), b.clone());
        g.add_edge(b.clone(), a.clone());

        assert!(g.has_any_edges_with(&a));
        assert!(g.has_any_edges_with(&b));

        g.clear();

        assert!(!g.has_any_edges_with(&a));
        assert!(!g.has_any_edges_with(&b));
        assert!(g.edges.is_empty());
    }

    // ---- Cycle detection -----------------------------------------------------

    #[test]
    fn test_detect_no_cycle() {
        let mut g = WaitForGraph::new();
        g.add_edge(t(1), t(2));
        g.add_edge(t(2), t(3));

        assert!(g.detect_cycle().is_none());
    }

    #[test]
    fn test_cycle_detection_simple_cycle() {
        let mut g = WaitForGraph::new();
        let a = Node::Task(1);
        let b = Node::Task(2);

        g.add_edge(a.clone(), b.clone());
        g.add_edge(b.clone(), a.clone());

        let cycle = g.detect_cycle().expect("cycle expected");
        // possible result: [Task(1), Task(2)]
        assert!(cycle.contains(&a));
        assert!(cycle.contains(&b));
    }

    #[test]
    fn test_detect_three_node_cycle() {
        let mut g = WaitForGraph::new();
        g.add_edge(t(1), t(2));
        g.add_edge(t(2), t(3));
        g.add_edge(t(3), t(1)); // 1 -> 2 -> 3 -> 1

        let cycle = g.detect_cycle().expect("expected a cycle");

        // Expect a 3-node cycle containing the three tasks (order may be rotated).
        assert_eq!(cycle.len(), 3);

        let expected: std::collections::HashSet<_> =
            std::iter::FromIterator::from_iter(vec![t(1), t(2), t(3)]);
        let found: std::collections::HashSet<_> = std::iter::FromIterator::from_iter(cycle.clone());
        assert_eq!(
            expected, found,
            "cycle must contain the three expected nodes"
        );

        // Also verify that each consecutive pair in the returned cycle is an actual directed edge
        // in the graph (including wrap-around).
        for i in 0..cycle.len() {
            let a = &cycle[i];
            let b = &cycle[(i + 1) % cycle.len()];
            let has_edge = g.edges.get(a).map(|s| s.contains(b)).unwrap_or(false);
            assert!(has_edge, "expected edge {:?} -> {:?} in cycle", a, b);
        }
    }

    #[test]
    fn test_detect_cycle_with_locks() {
        let mut g = WaitForGraph::new();

        // Task 1 waits for Lock L1 → L1 is held by Task 2 → Task 2 waits for Task 1
        g.add_edge(t(1), l("L1"));
        g.add_edge(l("L1"), t(2));
        g.add_edge(t(2), t(1));

        let cycle = g.detect_cycle().expect("expected cycle");
        assert_eq!(cycle.len(), 3);

        // Expected members regardless of order
        let expected: std::collections::HashSet<_> =
            std::iter::FromIterator::from_iter(vec![t(1), l("L1"), t(2)]);
        let found: std::collections::HashSet<_> = std::iter::FromIterator::from_iter(cycle.clone());
        assert_eq!(expected, found, "cycle must contain the expected nodes");

        // Ensure the returned cycle is valid: every adjacent pair must be an edge
        for i in 0..cycle.len() {
            let a = &cycle[i];
            let b = &cycle[(i + 1) % cycle.len()];
            let has_edge = g.edges.get(a).map(|set| set.contains(b)).unwrap_or(false);
            assert!(has_edge, "cycle edge missing: expected {:?} → {:?}", a, b);
        }
    }

    #[test]
    fn test_detects_only_one_cycle() {
        let mut g = WaitForGraph::new();

        // Two cycles exist: (1→2→1) and (3→4→3)
        g.add_edge(t(1), t(2));
        g.add_edge(t(2), t(1));

        g.add_edge(t(3), t(4));
        g.add_edge(t(4), t(3));

        let cycle = g.detect_cycle().unwrap();

        // must be a 2-node cycle
        assert_eq!(cycle.len(), 2);

        // Accept either {1,2} or {3,4} (order/rotation may vary)
        use std::collections::HashSet;
        let nodes: HashSet<_> = cycle.iter().cloned().collect();
        let ab: HashSet<_> = [t(1), t(2)].into_iter().collect();
        let cd: HashSet<_> = [t(3), t(4)].into_iter().collect();

        assert!(
            nodes == ab || nodes == cd,
            "unexpected cycle nodes: {nodes:?}, full cycle: {cycle:?}"
        );

        // Also verify the returned order corresponds to actual directed edges
        for i in 0..cycle.len() {
            let a = &cycle[i];
            let b = &cycle[(i + 1) % cycle.len()];
            assert!(
                g.has_edge(a, b),
                "expected edge {:?} -> {:?} in returned cycle",
                a,
                b
            );
        }
    }
}
