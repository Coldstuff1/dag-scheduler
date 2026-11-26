//! Back-ends that turn a [`Dag`] into other formats.

use std::fmt::Write;

use rt_schema::{Dag, NodeKind};

/// Renders `dag` as GraphViz DOT with WCET/period shown in each node's label.
///
/// Sensors are ellipses and tasks are boxes. The `kind`/`wcet`/`period`
/// attributes are kept too, so the output can be imported again.
pub fn to_dot(dag: &Dag) -> String {
    let mut out = String::new();
    writeln!(out, "digraph {} {{", quote(&dag.name)).unwrap();
    writeln!(out, "  node [fontname=\"DejaVu Sans\"];").unwrap();
    for node in &dag.nodes {
        let (shape, extra, timing) = match node.kind {
            NodeKind::Sensor { period } => (
                "ellipse",
                format!(", period={period}"),
                format!("wcet: {}\\nperiod: {period}", node.wcet),
            ),
            NodeKind::Task => ("box", String::new(), format!("wcet: {}", node.wcet)),
        };
        let kind = match node.kind {
            NodeKind::Sensor { .. } => "sensor",
            NodeKind::Task => "task",
        };
        writeln!(
            out,
            "  {} [kind={kind}, wcet={}{extra}, shape={shape}, label=\"{}\\n{timing}\"];",
            quote(&node.name),
            node.wcet,
            escape(&node.name),
        )
        .unwrap();
    }
    for &(from, to) in &dag.edges {
        writeln!(out, "  {} -> {};", quote(&dag.nodes[from].name), quote(&dag.nodes[to].name)).unwrap();
    }
    out.push_str("}\n");
    out
}

/// Escapes `"` for use inside a quoted DOT string (the only escape DOT defines;
/// backslashes pass through so Graphviz escapes like `\n` keep working).
fn escape(s: &str) -> String {
    s.replace('"', "\\\"")
}

fn quote(s: &str) -> String {
    format!("\"{}\"", escape(s))
}

#[cfg(test)]
mod tests {
    use rt_schema::parse_dot;

    use super::*;

    const SRC: &str = r#"digraph g {
        cam [kind=sensor, wcet=2, period=10];
        "my \"task\"" [wcet=5];
        cam -> "my \"task\"";
    }"#;

    #[test]
    fn labels_include_timing() {
        let dot = to_dot(&parse_dot(SRC).unwrap());
        assert!(dot.contains(r#"label="cam\nwcet: 2\nperiod: 10""#), "{dot}");
        assert!(dot.contains(r#""my \"task\"" [kind=task"#), "{dot}");
        assert!(dot.contains(r#"label="my \"task\"\nwcet: 5""#), "{dot}");
        assert!(dot.contains("shape=ellipse") && dot.contains("shape=box"));
    }

    #[test]
    fn output_reimports_to_the_same_dag() {
        let dag = parse_dot(SRC).unwrap();
        assert_eq!(parse_dot(&to_dot(&dag)).unwrap(), dag);
    }
}
