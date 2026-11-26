//! Format-agnostic in-memory model of a task DAG.
//!
//! A DAG has one or more sources and exactly one sink. Times are `i64` ticks.

use std::collections::{HashMap, HashSet, VecDeque};

use thiserror::Error;

/// Index of a node in [`Dag::nodes`].
pub type NodeIdx = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// Periodic source node.
    Sensor { period: i64 },
    /// Computation node.
    Task,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub kind: NodeKind,
    pub wcet: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dag {
    pub name: String,
    pub nodes: Vec<Node>,
    /// Dependency edges `(from, to)`: `to` consumes the output of `from`.
    pub edges: Vec<(NodeIdx, NodeIdx)>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("duplicate node name `{0}`")]
    DuplicateNode(String),
    #[error("node `{node}`: {what} must be > 0, got {value}")]
    NonPositive { node: String, what: &'static str, value: i64 },
    #[error("sensor `{0}` has incoming edges; sensors must be sources")]
    SensorHasDeps(String),
    #[error("edge refers to unknown node index {0}")]
    UnknownNode(NodeIdx),
    #[error("graph is empty")]
    Empty,
    #[error("graph must have exactly one sink, found {}: {}", .0.len(), .0.join(", "))]
    SinkCount(Vec<String>),
    #[error("graph contains a cycle through `{0}`")]
    Cycle(String),
}

impl Dag {
    /// Direct dependencies (predecessors) of every node, in edge order, deduplicated.
    pub fn predecessors(&self) -> Vec<Vec<NodeIdx>> {
        let mut preds = vec![Vec::new(); self.nodes.len()];
        for &(from, to) in &self.edges {
            if !preds[to].contains(&from) {
                preds[to].push(from);
            }
        }
        preds
    }

    /// Nodes in a topological order (Kahn's algorithm, stable w.r.t. node order).
    pub fn topo_order(&self) -> Result<Vec<NodeIdx>, ValidationError> {
        let n = self.nodes.len();
        let mut indegree = vec![0usize; n];
        let mut succs = vec![Vec::new(); n];
        for (to, preds) in self.predecessors().into_iter().enumerate() {
            indegree[to] = preds.len();
            for from in preds {
                succs[from].push(to);
            }
        }
        let mut ready: VecDeque<_> = (0..n).filter(|&i| indegree[i] == 0).collect();
        let mut order = Vec::with_capacity(n);
        while let Some(i) = ready.pop_front() {
            order.push(i);
            for &s in &succs[i] {
                indegree[s] -= 1;
                if indegree[s] == 0 {
                    ready.push_back(s);
                }
            }
        }
        if order.len() != n {
            let stuck = (0..n).find(|&i| indegree[i] > 0).expect("some node is stuck");
            return Err(ValidationError::Cycle(self.nodes[stuck].name.clone()));
        }
        Ok(order)
    }

    /// The unique node with no successors.
    pub fn sink(&self) -> Result<NodeIdx, ValidationError> {
        let has_succ: HashSet<NodeIdx> = self.edges.iter().map(|&(from, _)| from).collect();
        let sinks: Vec<NodeIdx> = (0..self.nodes.len()).filter(|i| !has_succ.contains(i)).collect();
        match sinks.as_slice() {
            [s] => Ok(*s),
            _ => Err(ValidationError::SinkCount(
                sinks.iter().map(|&i| self.nodes[i].name.clone()).collect(),
            )),
        }
    }

    /// Checks all structural invariants. Returns the topological order on success.
    pub fn validate(&self) -> Result<Vec<NodeIdx>, ValidationError> {
        if self.nodes.is_empty() {
            return Err(ValidationError::Empty);
        }
        let mut seen = HashMap::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if seen.insert(node.name.as_str(), i).is_some() {
                return Err(ValidationError::DuplicateNode(node.name.clone()));
            }
            let positive = |what, value| {
                if value > 0 {
                    Ok(())
                } else {
                    Err(ValidationError::NonPositive { node: node.name.clone(), what, value })
                }
            };
            positive("wcet", node.wcet)?;
            if let NodeKind::Sensor { period } = node.kind {
                positive("period", period)?;
            }
        }
        for &(from, to) in &self.edges {
            for idx in [from, to] {
                if idx >= self.nodes.len() {
                    return Err(ValidationError::UnknownNode(idx));
                }
            }
            if matches!(self.nodes[to].kind, NodeKind::Sensor { .. }) {
                return Err(ValidationError::SensorHasDeps(self.nodes[to].name.clone()));
            }
        }
        let order = self.topo_order()?;
        self.sink()?;
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, kind: NodeKind, wcet: i64) -> Node {
        Node { name: name.into(), kind, wcet }
    }

    /// cam, lidar -> fuse; cam -> detect -> fuse
    fn diamond() -> Dag {
        Dag {
            name: "g".into(),
            nodes: vec![
                node("fuse", NodeKind::Task, 3),
                node("detect", NodeKind::Task, 5),
                node("cam", NodeKind::Sensor { period: 10 }, 2),
                node("lidar", NodeKind::Sensor { period: 20 }, 1),
            ],
            edges: vec![(2, 1), (1, 0), (2, 0), (3, 0)],
        }
    }

    #[test]
    fn topo_order_respects_edges() {
        let dag = diamond();
        let order = dag.validate().unwrap();
        let pos = |i| order.iter().position(|&x| x == i).unwrap();
        for &(from, to) in &dag.edges {
            assert!(pos(from) < pos(to));
        }
        assert_eq!(dag.sink().unwrap(), 0);
    }

    #[test]
    fn rejects_multiple_sinks() {
        let mut dag = diamond();
        dag.edges.retain(|&e| e != (3, 0)); // lidar becomes a second sink
        assert_eq!(
            dag.validate(),
            Err(ValidationError::SinkCount(vec!["fuse".into(), "lidar".into()]))
        );
    }

    #[test]
    fn rejects_cycle() {
        let mut dag = diamond();
        dag.edges.push((0, 1));
        assert!(matches!(dag.validate(), Err(ValidationError::Cycle(_))));
    }

    #[test]
    fn rejects_sensor_with_deps() {
        let mut dag = diamond();
        dag.edges.push((3, 2));
        assert_eq!(dag.validate(), Err(ValidationError::SensorHasDeps("cam".into())));
    }

    #[test]
    fn rejects_non_positive_times() {
        let mut dag = diamond();
        dag.nodes[2].kind = NodeKind::Sensor { period: 0 };
        assert!(matches!(dag.validate(), Err(ValidationError::NonPositive { what: "period", .. })));
    }
}
