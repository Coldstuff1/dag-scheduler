//! Format-agnostic task-DAG model plus input frontends (currently GraphViz DOT).

pub mod dot;
pub mod model;

pub use dot::{DotError, parse_dot};
pub use model::{Dag, Node, NodeIdx, NodeKind, ValidationError};
