//! GraphViz DOT frontend.
//!
//! Conventions:
//! ```dot
//! digraph pipeline {
//!   cam    [kind=sensor, wcet=2, period=10];
//!   detect [kind=task, wcet=5];
//!   fuse   [wcet=3];                 // kind defaults to task
//!   cam -> detect -> fuse;           // chains are allowed
//!   cam -> fuse;
//! }
//! ```
//! - `wcet` is required on every node; `period` is required on sensors and
//!   forbidden on tasks. Values are integer ticks.
//! - `node [...]` default attributes apply to nodes first mentioned after them.
//! - Subgraphs/clusters are flattened; `a -> {b c}` expands to `a -> b; a -> c`.
//! - Other attributes (`label`, `shape`, ...) are ignored.

use std::{borrow::Cow, collections::HashMap};

use graphviz_rust::dot_structures::{
    Attribute, EdgeTy, Graph, GraphAttributes, Id, NodeId, Stmt, Vertex,
};
use thiserror::Error;

use crate::model::{Dag, Node, NodeIdx, NodeKind, ValidationError};

#[derive(Debug, Error)]
pub enum DotError {
    #[error("DOT syntax error: {0}")]
    Syntax(String),
    #[error("expected a `digraph`, found an undirected `graph`")]
    Undirected,
    #[error("node `{0}` is missing required attribute `{1}`")]
    MissingAttr(String, &'static str),
    #[error("node `{node}`: attribute `{attr}` must be an integer, got `{value}`")]
    NotAnInteger { node: String, attr: &'static str, value: String },
    #[error("node `{node}`: unknown kind `{kind}` (expected `sensor` or `task`)")]
    UnknownKind { node: String, kind: String },
    #[error("task `{0}` must not have a `period`")]
    PeriodOnTask(String),
    #[error(transparent)]
    Invalid(#[from] ValidationError),
}

/// Parses DOT source into a validated [`Dag`].
pub fn parse_dot(src: &str) -> Result<Dag, DotError> {
    let (name, stmts) = match graphviz_rust::parse(src).map_err(DotError::Syntax)? {
        Graph::DiGraph { id, stmts, .. } => (id, stmts),
        Graph::Graph { .. } => return Err(DotError::Undirected),
    };
    let mut builder = Builder::default();
    builder.stmts(&stmts, &mut Vec::new());

    let dag = Dag {
        name: match id_str(&name).as_ref() {
            "" => "dag".to_string(),
            s => s.to_string(),
        },
        nodes: builder.nodes.into_iter().map(raw_to_node).collect::<Result<_, _>>()?,
        edges: builder.edges,
    };
    dag.validate()?;
    Ok(dag)
}

/// A node as written in the file: name plus raw string attributes.
struct RawNode {
    name: String,
    attrs: HashMap<String, String>,
}

#[derive(Default)]
struct Builder {
    nodes: Vec<RawNode>,
    index: HashMap<String, NodeIdx>,
    edges: Vec<(NodeIdx, NodeIdx)>,
}

impl Builder {
    /// `defaults` holds the `node [...]` attributes in scope (copied into subgraphs).
    fn stmts(&mut self, stmts: &[Stmt], defaults: &mut Vec<(String, String)>) {
        for stmt in stmts {
            match stmt {
                Stmt::Node(node) => {
                    let idx = self.node(&node.id, defaults);
                    set_attrs(&mut self.nodes[idx].attrs, &node.attributes);
                }
                Stmt::Edge(edge) => {
                    let groups: Vec<Vec<NodeIdx>> = match &edge.ty {
                        EdgeTy::Pair(a, b) => vec![self.vertex(a, defaults), self.vertex(b, defaults)],
                        EdgeTy::Chain(vs) => vs.iter().map(|v| self.vertex(v, defaults)).collect(),
                    };
                    for pair in groups.windows(2) {
                        for &from in &pair[0] {
                            for &to in &pair[1] {
                                if !self.edges.contains(&(from, to)) {
                                    self.edges.push((from, to));
                                }
                            }
                        }
                    }
                }
                Stmt::Subgraph(sub) => self.stmts(&sub.stmts, &mut defaults.clone()),
                Stmt::GAttribute(GraphAttributes::Node(attrs)) => {
                    for Attribute(k, v) in attrs {
                        defaults.retain(|(dk, _)| *dk != id_str(k));
                        defaults.push((id_str(k).to_string(), id_str(v).to_string()));
                    }
                }
                Stmt::GAttribute(_) | Stmt::Attribute(_) => {}
            }
        }
    }

    /// Returns the indices of the nodes a vertex refers to, declaring them if new.
    fn vertex(&mut self, v: &Vertex, defaults: &[(String, String)]) -> Vec<NodeIdx> {
        match v {
            Vertex::N(id) => vec![self.node(id, defaults)],
            Vertex::S(sub) => {
                self.stmts(&sub.stmts, &mut defaults.to_vec());
                let mut names = Vec::new();
                mentioned(&sub.stmts, &mut names);
                let mut idxs: Vec<NodeIdx> = names.iter().map(|n| self.index[n.as_ref()]).collect();
                idxs.sort_unstable();
                idxs.dedup();
                idxs
            }
        }
    }

    fn node(&mut self, id: &NodeId, defaults: &[(String, String)]) -> NodeIdx {
        let name = id_str(&id.0);
        if let Some(&idx) = self.index.get(name.as_ref()) {
            return idx;
        }
        let idx = self.nodes.len();
        self.nodes.push(RawNode { name: name.to_string(), attrs: defaults.iter().cloned().collect() });
        self.index.insert(name.to_string(), idx);
        idx
    }
}

/// Names of all nodes mentioned (declared or used in edges) in `stmts`, recursively.
fn mentioned<'a>(stmts: &'a [Stmt], out: &mut Vec<Cow<'a, str>>) {
    let vertex = |v: &'a Vertex, out: &mut Vec<Cow<'a, str>>| match v {
        Vertex::N(id) => out.push(id_str(&id.0)),
        Vertex::S(sub) => mentioned(&sub.stmts, out),
    };
    for stmt in stmts {
        match stmt {
            Stmt::Node(n) => out.push(id_str(&n.id.0)),
            Stmt::Edge(e) => match &e.ty {
                EdgeTy::Pair(a, b) => {
                    vertex(a, out);
                    vertex(b, out);
                }
                EdgeTy::Chain(vs) => vs.iter().for_each(|v| vertex(v, out)),
            },
            Stmt::Subgraph(sub) => mentioned(&sub.stmts, out),
            Stmt::GAttribute(_) | Stmt::Attribute(_) => {}
        }
    }
}

fn set_attrs(dst: &mut HashMap<String, String>, attrs: &[Attribute]) {
    for Attribute(k, v) in attrs {
        dst.insert(id_str(k).to_string(), id_str(v).to_string());
    }
}

/// The textual content of a DOT ID. Quoted strings lose their quotes and `\"`
/// is unescaped (the only escape DOT defines; other backslashes are kept, since
/// Graphviz interprets them later, e.g. `\n` in labels).
fn id_str(id: &Id) -> Cow<'_, str> {
    match id {
        Id::Escaped(s) => {
            let inner = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s);
            if inner.contains("\\\"") {
                Cow::Owned(inner.replace("\\\"", "\""))
            } else {
                Cow::Borrowed(inner)
            }
        }
        Id::Html(s) | Id::Plain(s) | Id::Anonymous(s) => Cow::Borrowed(s),
    }
}

fn raw_to_node(raw: RawNode) -> Result<Node, DotError> {
    let int = |attr: &'static str| -> Result<Option<i64>, DotError> {
        raw.attrs
            .get(attr)
            .map(|v| {
                v.trim().parse().map_err(|_| DotError::NotAnInteger {
                    node: raw.name.clone(),
                    attr,
                    value: v.clone(),
                })
            })
            .transpose()
    };
    let wcet = int("wcet")?.ok_or_else(|| DotError::MissingAttr(raw.name.clone(), "wcet"))?;
    let period = int("period")?;
    let kind = match raw.attrs.get("kind").map(|k| k.to_ascii_lowercase()).as_deref() {
        None | Some("task") => match period {
            None => NodeKind::Task,
            Some(_) => return Err(DotError::PeriodOnTask(raw.name)),
        },
        Some("sensor") => NodeKind::Sensor {
            period: period.ok_or_else(|| DotError::MissingAttr(raw.name.clone(), "period"))?,
        },
        Some(other) => return Err(DotError::UnknownKind { node: raw.name, kind: other.to_string() }),
    };
    Ok(Node { name: raw.name, kind, wcet })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIPELINE: &str = r#"
        digraph pipeline {
          cam    [kind=sensor, wcet=2, period=10, shape=box];
          lidar  [kind="sensor", wcet="1", period=20];
          detect [kind=task, wcet=5];
          fuse   [wcet=3, label="fusion"];
          cam -> detect -> fuse;
          {cam lidar} -> fuse;
        }
    "#;

    fn find<'a>(dag: &'a Dag, name: &str) -> (NodeIdx, &'a Node) {
        dag.nodes.iter().enumerate().find(|(_, n)| n.name == name).unwrap()
    }

    #[test]
    fn parses_pipeline() {
        let dag = parse_dot(PIPELINE).unwrap();
        assert_eq!(dag.name, "pipeline");
        assert_eq!(dag.nodes.len(), 4);
        let (cam, cam_node) = find(&dag, "cam");
        let (lidar, lidar_node) = find(&dag, "lidar");
        let (detect, _) = find(&dag, "detect");
        let (fuse, fuse_node) = find(&dag, "fuse");
        assert_eq!(cam_node.kind, NodeKind::Sensor { period: 10 });
        assert_eq!(lidar_node.kind, NodeKind::Sensor { period: 20 });
        assert_eq!(fuse_node.kind, NodeKind::Task);
        assert_eq!(fuse_node.wcet, 3);
        let mut edges = dag.edges.clone();
        edges.sort();
        let mut expected = vec![(cam, detect), (detect, fuse), (cam, fuse), (lidar, fuse)];
        expected.sort();
        assert_eq!(edges, expected);
        assert_eq!(dag.sink().unwrap(), fuse);
    }

    #[test]
    fn node_defaults_apply() {
        let dag = parse_dot(
            "digraph { node [kind=sensor, period=5]; a [wcet=1]; b [wcet=1]; \
             node [kind=task, period=\"\"]; c [wcet=2]; a -> c; b -> c; }",
        );
        // An empty period on a task is still a period: reject so mistakes are loud.
        assert!(matches!(dag, Err(DotError::NotAnInteger { .. })));

        let dag = parse_dot(
            "digraph { node [kind=sensor, period=5]; a [wcet=1]; b [wcet=1]; \
             c [kind=task, wcet=2]; a -> c; b -> c; }",
        );
        // `c` inherits period=5 from the defaults.
        assert!(matches!(dag, Err(DotError::PeriodOnTask(n)) if n == "c"));
    }

    #[test]
    fn unescapes_quoted_ids() {
        let dag = parse_dot(r#"digraph "my \"g\"" { "a \"b\"" [wcet=1, label="x\ny"]; }"#).unwrap();
        assert_eq!(dag.name, r#"my "g""#);
        assert_eq!(dag.nodes[0].name, r#"a "b""#);
    }

    #[test]
    fn errors() {
        let cases: &[(&str, fn(&DotError) -> bool)] = &[
            ("graph { a -- b }", |e| matches!(e, DotError::Undirected)),
            ("digraph { a [wcet=1]; a -> b; }", |e| matches!(e, DotError::MissingAttr(n, "wcet") if n == "b")),
            ("digraph { a [kind=sensor, wcet=1]; }", |e| matches!(e, DotError::MissingAttr(n, "period") if n == "a")),
            ("digraph { a [wcet=x]; }", |e| matches!(e, DotError::NotAnInteger { attr: "wcet", .. })),
            ("digraph { a [kind=actor, wcet=1]; }", |e| matches!(e, DotError::UnknownKind { .. })),
            ("digraph { a [wcet=1, period=3]; }", |e| matches!(e, DotError::PeriodOnTask(_))),
            (
                "digraph { a [wcet=1]; b [kind=sensor, wcet=1, period=2]; a -> b; }",
                |e| matches!(e, DotError::Invalid(ValidationError::SensorHasDeps(_))),
            ),
            (
                "digraph { a [wcet=1]; b [wcet=1]; c [wcet=1]; a -> b; a -> c; }",
                |e| matches!(e, DotError::Invalid(ValidationError::SinkCount(_))),
            ),
            (
                "digraph { a [wcet=1]; b [wcet=1]; a -> b -> a; }",
                |e| matches!(e, DotError::Invalid(ValidationError::Cycle(_))),
            ),
            ("digraph { a [wcet=1 ", |e| matches!(e, DotError::Syntax(_))),
        ];
        for (src, check) in cases {
            let err = parse_dot(src).expect_err(src);
            assert!(check(&err), "{src}: unexpected error {err:?}");
        }
    }
}
