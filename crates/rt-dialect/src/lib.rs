//! The `rt` MLIR dialect, defined in IRDL (`irdl/rt.irdl.mlir`) and loaded at runtime.
//!
//! A DAG is a `func.func` tagged with the `rt.graph` unit attribute. Its body
//! holds one `rt.sensor` / `rt.task` op per node; every node yields a
//! `!rt.token` and tasks take their dependencies' tokens as operands. The
//! function returns the sink's token. A `func` body is an SSACFG region, so
//! dominance forces the nodes into topological order (cycles fail to verify).
//! The builtin `module` is a graph region and would not check this, and IRDL
//! can't give a custom op the `NoTerminator` trait, hence `func.func`.
//!
//! ```mlir
//! func.func @pipeline() -> !rt.token attributes {rt.graph} {
//!   %0 = "rt.sensor"() {name = "cam", period = 10 : i64, wcet = 2 : i64} : () -> !rt.token
//!   %1 = "rt.task"(%0) {name = "detect", wcet = 5 : i64} : (!rt.token) -> !rt.token
//!   return %1 : !rt.token
//! }
//! ```

use std::collections::HashSet;

use melior::{
    Context,
    dialect::{DialectRegistry, func},
    ir::{
        Attribute, AttributeLike, Block, BlockLike, Identifier, Location, Module, Operation, Region,
        RegionLike, Type, Value,
        attribute::{IntegerAttribute, StringAttribute, TypeAttribute},
        operation::{OperationBuilder, OperationLike, WalkOrder, WalkResult},
        r#type::{FunctionType, IntegerType},
    },
    utility::{load_irdl_dialects, register_all_dialects},
};
use rt_schema::{Dag, NodeKind, ValidationError};
use thiserror::Error;

/// IRDL source of the `rt` dialect.
pub const RT_IRDL: &str = include_str!("../irdl/rt.irdl.mlir");

/// Op / attribute names.
pub const SENSOR_OP: &str = "rt.sensor";
pub const TASK_OP: &str = "rt.task";
pub const GRAPH_ATTR: &str = "rt.graph";

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to parse or load the rt IRDL definition")]
    LoadDialect,
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("generated IR failed MLIR verification")]
    Verify,
    #[error("rt verification failed:\n  {}", .0.join("\n  "))]
    RtVerify(Vec<String>),
}

/// A context with all upstream dialects registered and `rt` loaded.
/// Unregistered dialects are rejected.
pub fn create_context() -> Result<Context, Error> {
    let registry = DialectRegistry::new();
    register_all_dialects(&registry);

    let context = Context::new();
    context.append_dialect_registry(&registry);
    context.load_all_available_dialects();
    load_rt_dialect(&context)?;
    context.set_allow_unregistered_dialects(false);
    Ok(context)
}

/// Parses [`RT_IRDL`] and registers the `rt` dialect in `context`.
/// The IRDL module can be dropped afterwards: the dynamic dialect keeps its own
/// copy of the constraints.
pub fn load_rt_dialect(context: &Context) -> Result<(), Error> {
    let irdl = Module::parse(context, RT_IRDL).ok_or(Error::LoadDialect)?;
    load_irdl_dialects(&irdl).then_some(()).ok_or(Error::LoadDialect)
}

/// `!rt.token`
pub fn token_type(context: &Context) -> Type<'_> {
    Type::parse(context, "!rt.token").expect("rt dialect must be loaded")
}

fn i64_attr(context: &Context, value: i64) -> Attribute<'_> {
    IntegerAttribute::new(IntegerType::new(context, 64).into(), value).into()
}

fn named<'c>(context: &'c Context, name: &str, attr: Attribute<'c>) -> (Identifier<'c>, Attribute<'c>) {
    (Identifier::new(context, name), attr)
}

/// `%t = "rt.sensor"() {name, wcet, period} : () -> !rt.token`
pub fn sensor<'c>(
    context: &'c Context,
    location: Location<'c>,
    name: &str,
    wcet: i64,
    period: i64,
) -> Operation<'c> {
    OperationBuilder::new(SENSOR_OP, location)
        .add_attributes(&[
            named(context, "name", StringAttribute::new(context, name).into()),
            named(context, "wcet", i64_attr(context, wcet)),
            named(context, "period", i64_attr(context, period)),
        ])
        .add_results(&[token_type(context)])
        .build()
        .expect("valid rt.sensor")
}

/// `%t = "rt.task"(%deps...) {name, wcet} : (!rt.token...) -> !rt.token`
pub fn task<'c>(
    context: &'c Context,
    location: Location<'c>,
    name: &str,
    wcet: i64,
    deps: &[Value<'c, '_>],
) -> Operation<'c> {
    OperationBuilder::new(TASK_OP, location)
        .add_attributes(&[
            named(context, "name", StringAttribute::new(context, name).into()),
            named(context, "wcet", i64_attr(context, wcet)),
        ])
        .add_operands(deps)
        .add_results(&[token_type(context)])
        .build()
        .expect("valid rt.task")
}

/// Lowers a [`Dag`] to a module holding one `rt.graph` function, and verifies it.
pub fn build_module<'c>(context: &'c Context, dag: &Dag) -> Result<Module<'c>, Error> {
    let order = dag.validate()?;
    let sink = dag.sink()?;
    let preds = dag.predecessors();
    let location = Location::unknown(context);
    let token = token_type(context);

    let region = Region::new();
    {
        let block = region.append_block(Block::new(&[]));
        let mut values: Vec<Option<Value>> = vec![None; dag.nodes.len()];
        for i in order {
            let node = &dag.nodes[i];
            let op = match node.kind {
                NodeKind::Sensor { period } => sensor(context, location, &node.name, node.wcet, period),
                NodeKind::Task => {
                    // Topological order guarantees every dependency is already built.
                    let deps: Vec<Value> = preds[i].iter().map(|&p| values[p].unwrap()).collect();
                    task(context, location, &node.name, node.wcet, &deps)
                }
            };
            values[i] = Some(block.append_operation(op).result(0).unwrap().into());
        }
        block.append_operation(func::r#return(&[values[sink].unwrap()], location));
    }

    let module = Module::new(location);
    module.body().append_operation(func::func(
        context,
        StringAttribute::new(context, &dag.name),
        TypeAttribute::new(FunctionType::new(context, &[], &[token]).into()),
        region,
        &[named(context, GRAPH_ATTR, Attribute::unit(context))],
        location,
    ));

    if !module.as_operation().verify() {
        return Err(Error::Verify);
    }
    verify_rt(&module)?;
    Ok(module)
}

/// Checks the constraints IRDL can't express: `wcet`/`period` are `i64` and
/// strictly positive, and node names are unique within each graph.
pub fn verify_rt(module: &Module) -> Result<(), Error> {
    let mut errors = Vec::new();
    let mut names: HashSet<(String, String)> = HashSet::new();
    module.as_operation().walk(WalkOrder::PreOrder, |op| {
        let op_name = op.name().as_string_ref().as_str().unwrap_or_default().to_string();
        let time_attrs: &[&str] = match op_name.as_str() {
            SENSOR_OP => &["wcet", "period"],
            TASK_OP => &["wcet"],
            _ => return WalkResult::Advance,
        };
        let node = op
            .attribute("name")
            .ok()
            .and_then(|a| StringAttribute::try_from(a).ok())
            .map(|s| s.value().to_string())
            .unwrap_or_default();
        let graph = op
            .parent_operation()
            .and_then(|p| p.attribute("sym_name").ok())
            .and_then(|a| StringAttribute::try_from(a).ok())
            .map(|s| s.value().to_string())
            .unwrap_or_default();
        if !names.insert((graph.clone(), node.clone())) {
            errors.push(format!("@{graph}: duplicate node name `{node}`"));
        }
        for &attr in time_attrs {
            match op.attribute(attr).ok().and_then(|a| IntegerAttribute::try_from(a).ok()) {
                Some(v) if v.r#type().to_string() != "i64" => {
                    errors.push(format!("{op_name} `{node}`: `{attr}` must be i64"))
                }
                Some(v) if v.value() <= 0 => {
                    errors.push(format!("{op_name} `{node}`: `{attr}` must be > 0, got {}", v.value()))
                }
                Some(_) => {}
                None => errors.push(format!("{op_name} `{node}`: missing integer `{attr}`")),
            }
        }
        WalkResult::Advance
    });
    if errors.is_empty() { Ok(()) } else { Err(Error::RtVerify(errors)) }
}

#[cfg(test)]
mod tests {
    use rt_schema::Node;

    use super::*;

    fn pipeline() -> Dag {
        let node = |name: &str, kind, wcet| Node { name: name.into(), kind, wcet };
        Dag {
            name: "pipeline".into(),
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
    fn builds_and_verifies_pipeline() {
        let context = create_context().unwrap();
        let module = build_module(&context, &pipeline()).unwrap();
        let ir = module.as_operation().to_string();
        println!("{ir}");
        assert!(ir.contains("func.func @pipeline() -> !rt.token attributes {rt.graph}"));
        assert!(ir.contains(r#""rt.sensor"() {name = "cam", period = 10 : i64, wcet = 2 : i64}"#));
        assert!(ir.contains(
            r#""rt.task"(%2, %0, %1) {name = "fuse", wcet = 3 : i64} : (!rt.token, !rt.token, !rt.token) -> !rt.token"#
        ));
        assert!(ir.contains("return %3 : !rt.token"));
    }

    fn parse<'c>(context: &'c Context, body: &str) -> Option<Module<'c>> {
        Module::parse(
            context,
            &format!("func.func @g() -> !rt.token attributes {{rt.graph}} {{\n{body}\n}}"),
        )
    }

    #[test]
    fn rejects_cycle() {
        let context = create_context().unwrap();
        let module = parse(
            &context,
            r#"%a = "rt.task"(%b) {name = "a", wcet = 1 : i64} : (!rt.token) -> !rt.token
               %b = "rt.task"(%a) {name = "b", wcet = 1 : i64} : (!rt.token) -> !rt.token
               return %b : !rt.token"#,
        );
        // The parser itself runs the verifier, so the module is rejected outright.
        assert!(module.is_none());
    }

    #[test]
    fn rejects_missing_wcet_and_sensor_operands() {
        let context = create_context().unwrap();
        assert!(parse(&context, r#"%a = "rt.task"() {name = "a"} : () -> !rt.token
                                   return %a : !rt.token"#).is_none());
        assert!(parse(&context, r#"%s = "rt.sensor"() {name = "s", wcet = 1 : i64, period = 2 : i64} : () -> !rt.token
                                   %t = "rt.sensor"(%s) {name = "t", wcet = 1 : i64, period = 2 : i64} : (!rt.token) -> !rt.token
                                   return %t : !rt.token"#).is_none());
    }

    #[test]
    fn verify_rt_catches_what_irdl_cannot() {
        let context = create_context().unwrap();
        let module = parse(
            &context,
            r#"%s = "rt.sensor"() {name = "s", wcet = 1 : i32, period = 0 : i64} : () -> !rt.token
               %t = "rt.task"(%s) {name = "s", wcet = -4 : i64} : (!rt.token) -> !rt.token
               return %t : !rt.token"#,
        )
        .expect("IRDL accepts these");
        let Err(Error::RtVerify(errors)) = verify_rt(&module) else { panic!("expected errors") };
        assert_eq!(errors.len(), 4, "{errors:#?}");
    }
}
