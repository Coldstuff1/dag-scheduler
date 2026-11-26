use melior::{
    Context,
    dialect::DialectRegistry,
    ir::{
        BlockLike, Identifier, Location, Module,
        attribute::IntegerAttribute,
        operation::OperationBuilder,
        r#type::IntegerType,
    },
    utility::register_all_dialects,
};

pub fn create_context() -> Context {
    let registry = DialectRegistry::new();
    register_all_dialects(&registry);

    let context = Context::new();
    context.append_dialect_registry(&registry);
    context.load_all_available_dialects();
    context.set_allow_unregistered_dialects(true);
    context
}

#[cfg(test)]
mod tests {
    use melior::ir::operation::OperationLike;

use super::*;

    #[test]
    fn smoke_test_rt_task_op() {
        let context = create_context();
        let location = Location::unknown(&context);
        let module = Module::new(location);

        let i32_type = IntegerType::new(&context, 32).into();
        let wcet_attr = IntegerAttribute::new(i32_type, 13).into();

        let op = OperationBuilder::new("rt.task", location)
            .add_attributes(&[(Identifier::new(&context, "wcet"), wcet_attr)])
            .build()
            .expect("failed to build rt.task op");

        module.body().append_operation(op);

        assert!(module.as_operation().verify());

        let ir = module.as_operation().to_string();
        println!("{ir}");
        assert!(ir.contains("\"rt.task\""));
        assert!(ir.contains("wcet = 13 : i32"));
    }
}
