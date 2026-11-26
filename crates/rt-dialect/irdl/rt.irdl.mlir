// IRDL definition of the `rt` dialect: a program as a DAG of sensors and tasks.
//
// Edges are SSA values of type !rt.token: every node produces one token and a
// task consumes the tokens of the nodes it depends on. Because nodes live in an
// SSACFG region (the builtin module), MLIR dominance forces them to appear in
// topological order, so a cyclic graph cannot be expressed.
//
// All times (wcet, period) are integer attributes in abstract ticks. IRDL only
// checks they are #builtin.integer; the Rust verifier (rt_dialect::verify)
// additionally requires i64 and strictly positive values.
//
// Note: in IRDL, a constraint value used twice must match the *same* attribute
// value both times, so each attribute gets its own constraint op.
irdl.dialect @rt {
  // Dependency token produced by every node.
  irdl.type @token {
    irdl.parameters()
  }

  // Periodic source node: has a WCET and a release period, no dependencies.
  irdl.operation @sensor {
    %name = irdl.base "#builtin.string"
    %wcet = irdl.base "#builtin.integer"
    %period = irdl.base "#builtin.integer"
    %tok = irdl.base @rt::@token
    irdl.attributes {
      "name" = %name,
      "wcet" = %wcet,
      "period" = %period
    }
    irdl.operands()
    irdl.results(out: %tok)
  }

  // Computation node: has a WCET and depends on zero or more other nodes.
  irdl.operation @task {
    %name = irdl.base "#builtin.string"
    %wcet = irdl.base "#builtin.integer"
    %tok = irdl.base @rt::@token
    irdl.attributes {
      "name" = %name,
      "wcet" = %wcet
    }
    irdl.operands(deps: variadic %tok)
    irdl.results(out: %tok)
  }
}
