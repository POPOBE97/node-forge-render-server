use super::source_mapping::{source_range_from_span, target_source_range};
use super::*;

pub fn module_to_ast_tree(
    module: &Module,
    source: &str,
    targets: &[PassDebugDependencyTarget],
) -> Vec<PassDebugAstNode> {
    vec![
        entry_points_node(module, source, targets),
        functions_node(module, source, targets),
        globals_node(module, targets),
        types_and_constants_node(module, source),
    ]
}

pub(super) fn function_scope_for_handle(
    handle: naga::Handle<Function>,
    function: &Function,
) -> String {
    function
        .name
        .clone()
        .unwrap_or_else(|| format!("function_{}", handle.index()))
}

pub(super) fn target_id_global(handle: naga::Handle<naga::GlobalVariable>) -> String {
    format!("global::{}", handle.index())
}

pub(super) fn target_id_arg(scope: &str, index: u32) -> String {
    format!("{scope}::arg::{index}")
}

pub(super) fn target_id_local(scope: &str, handle: naga::Handle<naga::LocalVariable>) -> String {
    format!("{scope}::local::{}", handle.index())
}

pub(super) fn target_id_expr(scope: &str, handle: naga::Handle<Expression>) -> String {
    format!("{scope}::expr::{}", handle.index())
}

pub(super) fn target_id_return(scope: &str) -> String {
    format!("{scope}::return")
}

pub(super) fn entry_points_node(
    module: &Module,
    source: &str,
    targets: &[PassDebugDependencyTarget],
) -> PassDebugAstNode {
    let children = module
        .entry_points
        .iter()
        .map(|entry| {
            let scope = entry.name.as_str();
            let mut children = function_children(scope, &entry.function, source, targets);
            children.insert(
                0,
                PassDebugAstNode::leaf(format!("workgroup_size: {:?}", entry.workgroup_size)),
            );
            children.insert(
                0,
                PassDebugAstNode::leaf(format!("early_depth_test: {:?}", entry.early_depth_test)),
            );
            PassDebugAstNode::branch(format!("{:?} {}", entry.stage, entry.name), children)
        })
        .collect();
    PassDebugAstNode::branch(
        format!("Entry Points ({})", module.entry_points.len()),
        children,
    )
}

pub(super) fn functions_node(
    module: &Module,
    source: &str,
    targets: &[PassDebugDependencyTarget],
) -> PassDebugAstNode {
    let children = module
        .functions
        .iter()
        .map(|(handle, function)| {
            let scope = function_scope_for_handle(handle, function);
            PassDebugAstNode::branch(
                format!(
                    "{:?} {}",
                    handle,
                    function.name.as_deref().unwrap_or("<anonymous function>")
                ),
                function_children(&scope, function, source, targets),
            )
        })
        .collect();
    PassDebugAstNode::branch(format!("Functions ({})", module.functions.len()), children)
}

pub(super) fn globals_node(
    module: &Module,
    targets: &[PassDebugDependencyTarget],
) -> PassDebugAstNode {
    let children = module
        .global_variables
        .iter()
        .map(|(handle, global)| {
            let target_id = target_id_global(handle);
            PassDebugAstNode::branch(
                format!(
                    "{:?} {}",
                    handle,
                    global.name.as_deref().unwrap_or("<anonymous global>")
                ),
                vec![
                    PassDebugAstNode::leaf(format!("space: {:?}", global.space)),
                    PassDebugAstNode::leaf(format!("binding: {:?}", global.binding)),
                    PassDebugAstNode::leaf(format!("type: {:?}", global.ty)),
                    PassDebugAstNode::leaf(format!("init: {:?}", global.init)),
                ],
            )
            .with_target_range(
                target_id.clone(),
                "global",
                target_source_range(targets, &target_id),
            )
        })
        .collect();
    PassDebugAstNode::branch(
        format!("Globals ({})", module.global_variables.len()),
        children,
    )
}

pub(super) fn types_and_constants_node(module: &Module, source: &str) -> PassDebugAstNode {
    let type_children = module
        .types
        .iter()
        .map(|(handle, ty)| {
            PassDebugAstNode::branch(
                format!(
                    "{:?} {}",
                    handle,
                    ty.name.as_deref().unwrap_or("<anonymous type>")
                ),
                vec![PassDebugAstNode::leaf(format!("{:?}", ty.inner))],
            )
        })
        .collect();

    let constant_children = module
        .constants
        .iter()
        .map(|(handle, constant)| {
            PassDebugAstNode::branch(
                format!(
                    "{:?} {}",
                    handle,
                    constant.name.as_deref().unwrap_or("<anonymous constant>")
                ),
                vec![
                    PassDebugAstNode::leaf(format!("type: {:?}", constant.ty)),
                    PassDebugAstNode::leaf(format!("init: {:?}", constant.init)),
                ],
            )
        })
        .collect();

    let override_children = module
        .overrides
        .iter()
        .map(|(handle, override_const)| {
            PassDebugAstNode::branch(
                format!(
                    "{:?} {}",
                    handle,
                    override_const
                        .name
                        .as_deref()
                        .unwrap_or("<anonymous override>")
                ),
                vec![
                    PassDebugAstNode::leaf(format!("type: {:?}", override_const.ty)),
                    PassDebugAstNode::leaf(format!("init: {:?}", override_const.init)),
                    PassDebugAstNode::leaf(format!("id: {:?}", override_const.id)),
                ],
            )
        })
        .collect();

    PassDebugAstNode::branch(
        "Types / Constants",
        vec![
            PassDebugAstNode::branch(format!("Types ({})", module.types.len()), type_children),
            PassDebugAstNode::branch(
                format!("Constants ({})", module.constants.len()),
                constant_children,
            ),
            PassDebugAstNode::branch(
                format!("Overrides ({})", module.overrides.len()),
                override_children,
            ),
            expressions_group_node("Global Expressions", &module.global_expressions, source),
        ],
    )
}

pub(super) fn function_children(
    scope: &str,
    function: &Function,
    source: &str,
    targets: &[PassDebugDependencyTarget],
) -> Vec<PassDebugAstNode> {
    vec![
        PassDebugAstNode::branch(
            format!("Arguments ({})", function.arguments.len()),
            function
                .arguments
                .iter()
                .enumerate()
                .map(|(index, arg)| {
                    let target_id = target_id_arg(scope, index as u32);
                    PassDebugAstNode::branch(
                        format!(
                            "{}: {}",
                            index,
                            arg.name.as_deref().unwrap_or("<anonymous argument>")
                        ),
                        vec![
                            PassDebugAstNode::leaf(format!("type: {:?}", arg.ty)),
                            PassDebugAstNode::leaf(format!("binding: {:?}", arg.binding)),
                        ],
                    )
                    .with_target_range(
                        target_id.clone(),
                        "argument",
                        target_source_range(targets, &target_id),
                    )
                })
                .collect(),
        ),
        PassDebugAstNode::branch(
            "Result",
            match function.result.as_ref() {
                Some(result) => vec![
                    PassDebugAstNode::leaf(format!("type: {:?}", result.ty)),
                    PassDebugAstNode::leaf(format!("binding: {:?}", result.binding)),
                ],
                None => vec![PassDebugAstNode::leaf("none")],
            },
        ),
        PassDebugAstNode::branch(
            format!("Local Variables ({})", function.local_variables.len()),
            function
                .local_variables
                .iter()
                .map(|(handle, local)| {
                    let target_id = target_id_local(scope, handle);
                    PassDebugAstNode::branch(
                        format!(
                            "{:?} {}",
                            handle,
                            local.name.as_deref().unwrap_or("<anonymous local>")
                        ),
                        vec![
                            PassDebugAstNode::leaf(format!("type: {:?}", local.ty)),
                            PassDebugAstNode::leaf(format!("init: {:?}", local.init)),
                        ],
                    )
                    .with_target_range(
                        target_id.clone(),
                        "local",
                        target_source_range(targets, &target_id),
                    )
                })
                .collect(),
        ),
        expressions_group_node_with_named("Expressions", scope, function, source, targets),
        PassDebugAstNode::branch(
            "Body",
            block_to_nodes(&function.body, &function.expressions, 0, source),
        ),
    ]
}

pub(super) fn expressions_group_node(
    label: &str,
    expressions: &Arena<Expression>,
    source: &str,
) -> PassDebugAstNode {
    expressions_group_node_inner(label, expressions, source, None, &HashSet::new(), &[])
}

pub(super) fn expressions_group_node_with_named(
    label: &str,
    scope: &str,
    function: &Function,
    source: &str,
    targets: &[PassDebugDependencyTarget],
) -> PassDebugAstNode {
    let named_handles = function
        .named_expressions
        .keys()
        .copied()
        .collect::<HashSet<_>>();
    expressions_group_node_inner(
        label,
        &function.expressions,
        source,
        Some(scope),
        &named_handles,
        targets,
    )
}

pub(super) fn expressions_group_node_inner(
    label: &str,
    expressions: &Arena<Expression>,
    source: &str,
    scope: Option<&str>,
    named_handles: &HashSet<naga::Handle<Expression>>,
    targets: &[PassDebugDependencyTarget],
) -> PassDebugAstNode {
    PassDebugAstNode::branch(
        format!("{label} ({})", expressions.len()),
        expressions
            .iter()
            .map(|(handle, expr)| {
                let source_range = source_range_from_span(source, expressions.get_span(handle));
                let node = PassDebugAstNode::branch(
                    expression_label(handle, expr),
                    expression_children(expr, expressions, source, 0),
                )
                .with_source_range(source_range);
                if let Some(scope) = scope
                    && named_handles.contains(&handle)
                {
                    let target_id = target_id_expr(scope, handle);
                    node.with_target_range(
                        target_id.clone(),
                        "let",
                        target_source_range(targets, &target_id).or(source_range),
                    )
                } else {
                    node
                }
            })
            .collect(),
    )
}

pub(super) fn block_to_nodes(
    block: &Block,
    expressions: &Arena<Expression>,
    depth: usize,
    source: &str,
) -> Vec<PassDebugAstNode> {
    block
        .iter()
        .enumerate()
        .map(|(index, stmt)| {
            let label = format!("{index}: {}", statement_kind_label(stmt));
            PassDebugAstNode::branch(label, statement_children(stmt, expressions, source, depth))
        })
        .collect()
}

pub(super) fn statement_kind_label(stmt: &Statement) -> String {
    match stmt {
        Statement::Emit(range) => format!("Emit {range:?}"),
        Statement::Block(_) => "Block".to_string(),
        Statement::If { .. } => "If".to_string(),
        Statement::Switch { .. } => "Switch".to_string(),
        Statement::Loop { .. } => "Loop".to_string(),
        Statement::Break => "Break".to_string(),
        Statement::Continue => "Continue".to_string(),
        Statement::Return { .. } => "Return".to_string(),
        Statement::Kill => "Kill".to_string(),
        Statement::Barrier(barrier) => format!("Barrier {barrier:?}"),
        Statement::Store { .. } => "Store".to_string(),
        Statement::ImageStore { .. } => "ImageStore".to_string(),
        Statement::Atomic { fun, .. } => format!("Atomic {fun:?}"),
        Statement::WorkGroupUniformLoad { .. } => "WorkGroupUniformLoad".to_string(),
        Statement::Call { function, .. } => format!("Call {function:?}"),
        Statement::RayQuery { fun, .. } => format!("RayQuery {fun:?}"),
        Statement::SubgroupBallot { .. } => "SubgroupBallot".to_string(),
        Statement::SubgroupGather { mode, .. } => format!("SubgroupGather {mode:?}"),
        Statement::SubgroupCollectiveOperation {
            op, collective_op, ..
        } => format!("SubgroupCollectiveOperation {op:?}/{collective_op:?}"),
    }
}

pub(super) fn statement_children(
    stmt: &Statement,
    expressions: &Arena<Expression>,
    source: &str,
    depth: usize,
) -> Vec<PassDebugAstNode> {
    match stmt {
        Statement::Emit(range) => range
            .clone()
            .map(|handle| expression_node(handle, expressions, source, depth + 1))
            .collect(),
        Statement::Block(block) => block_to_nodes(block, expressions, depth + 1, source),
        Statement::If {
            condition,
            accept,
            reject,
        } => vec![
            PassDebugAstNode::branch(
                "condition",
                vec![expression_node(*condition, expressions, source, depth + 1)],
            ),
            PassDebugAstNode::branch(
                "accept",
                block_to_nodes(accept, expressions, depth + 1, source),
            ),
            PassDebugAstNode::branch(
                "reject",
                block_to_nodes(reject, expressions, depth + 1, source),
            ),
        ],
        Statement::Switch { selector, cases } => {
            let mut children = vec![PassDebugAstNode::branch(
                "selector",
                vec![expression_node(*selector, expressions, source, depth + 1)],
            )];
            children.extend(cases.iter().map(|case| {
                PassDebugAstNode::branch(
                    format!("case {:?} fall_through={}", case.value, case.fall_through),
                    block_to_nodes(&case.body, expressions, depth + 1, source),
                )
            }));
            children
        }
        Statement::Loop {
            body,
            continuing,
            break_if,
        } => vec![
            PassDebugAstNode::branch("body", block_to_nodes(body, expressions, depth + 1, source)),
            PassDebugAstNode::branch(
                "continuing",
                block_to_nodes(continuing, expressions, depth + 1, source),
            ),
            PassDebugAstNode::branch(
                "break_if",
                break_if
                    .map(|expr| vec![expression_node(expr, expressions, source, depth + 1)])
                    .unwrap_or_else(|| vec![PassDebugAstNode::leaf("none")]),
            ),
        ],
        Statement::Return { value } => value
            .map(|expr| vec![expression_node(expr, expressions, source, depth + 1)])
            .unwrap_or_default(),
        Statement::Store { pointer, value } => vec![
            PassDebugAstNode::branch(
                "pointer",
                vec![expression_node(*pointer, expressions, source, depth + 1)],
            ),
            PassDebugAstNode::branch(
                "value",
                vec![expression_node(*value, expressions, source, depth + 1)],
            ),
        ],
        Statement::ImageStore {
            image,
            coordinate,
            array_index,
            value,
        } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[
                ("image", Some(*image)),
                ("coordinate", Some(*coordinate)),
                ("array_index", *array_index),
                ("value", Some(*value)),
            ],
        ),
        Statement::Atomic {
            pointer,
            value,
            result,
            ..
        } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[
                ("pointer", Some(*pointer)),
                ("value", Some(*value)),
                ("result", Some(*result)),
            ],
        ),
        Statement::WorkGroupUniformLoad { pointer, result } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[("pointer", Some(*pointer)), ("result", Some(*result))],
        ),
        Statement::Call {
            arguments, result, ..
        } => {
            let mut children: Vec<_> = arguments
                .iter()
                .enumerate()
                .map(|(index, arg)| {
                    PassDebugAstNode::branch(
                        format!("arg {index}"),
                        vec![expression_node(*arg, expressions, source, depth + 1)],
                    )
                })
                .collect();
            if let Some(result) = result {
                children.push(PassDebugAstNode::branch(
                    "result",
                    vec![expression_node(*result, expressions, source, depth + 1)],
                ));
            }
            children
        }
        Statement::RayQuery { query, .. } => {
            vec![PassDebugAstNode::branch(
                "query",
                vec![expression_node(*query, expressions, source, depth + 1)],
            )]
        }
        Statement::SubgroupBallot { result, predicate } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[("result", Some(*result)), ("predicate", *predicate)],
        ),
        Statement::SubgroupGather {
            argument, result, ..
        } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[("argument", Some(*argument)), ("result", Some(*result))],
        ),
        Statement::SubgroupCollectiveOperation {
            argument, result, ..
        } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[("argument", Some(*argument)), ("result", Some(*result))],
        ),
        other => vec![PassDebugAstNode::leaf(format!("{other:#?}"))],
    }
}

pub(super) fn expression_node(
    handle: naga::Handle<Expression>,
    expressions: &Arena<Expression>,
    source: &str,
    depth: usize,
) -> PassDebugAstNode {
    let expr = &expressions[handle];
    PassDebugAstNode::branch(
        expression_label(handle, expr),
        expression_children(expr, expressions, source, depth),
    )
    .with_source_range(source_range_from_span(source, expressions.get_span(handle)))
}

pub(super) fn expression_label(handle: naga::Handle<Expression>, expr: &Expression) -> String {
    format!("{handle:?}: {}", expression_kind_label(expr))
}

pub(super) fn expression_kind_label(expr: &Expression) -> String {
    match expr {
        Expression::Literal(lit) => format!("Literal {lit:?}"),
        Expression::Constant(handle) => format!("Constant {handle:?}"),
        Expression::Override(handle) => format!("Override {handle:?}"),
        Expression::ZeroValue(handle) => format!("ZeroValue {handle:?}"),
        Expression::Compose { ty, .. } => format!("Compose {ty:?}"),
        Expression::Access { .. } => "Access".to_string(),
        Expression::AccessIndex { index, .. } => format!("AccessIndex {index}"),
        Expression::Splat { size, .. } => format!("Splat {size:?}"),
        Expression::Swizzle { size, pattern, .. } => format!("Swizzle {size:?} {pattern:?}"),
        Expression::FunctionArgument(index) => format!("FunctionArgument {index}"),
        Expression::GlobalVariable(handle) => format!("GlobalVariable {handle:?}"),
        Expression::LocalVariable(handle) => format!("LocalVariable {handle:?}"),
        Expression::Load { .. } => "Load".to_string(),
        Expression::ImageSample { level, .. } => format!("ImageSample {level:?}"),
        Expression::ImageLoad { .. } => "ImageLoad".to_string(),
        Expression::ImageQuery { query, .. } => format!("ImageQuery {query:?}"),
        Expression::Unary { op, .. } => format!("Unary {op:?}"),
        Expression::Binary { op, .. } => format!("Binary {op:?}"),
        Expression::Select { .. } => "Select".to_string(),
        Expression::Derivative { axis, ctrl, .. } => format!("Derivative {axis:?}/{ctrl:?}"),
        Expression::Relational { fun, .. } => format!("Relational {fun:?}"),
        Expression::Math { fun, .. } => format!("Math {fun:?}"),
        Expression::As { kind, convert, .. } => format!("As {kind:?} convert={convert:?}"),
        Expression::CallResult(handle) => format!("CallResult {handle:?}"),
        Expression::AtomicResult { ty, comparison } => {
            format!("AtomicResult {ty:?} comparison={comparison}")
        }
        Expression::WorkGroupUniformLoadResult { ty } => {
            format!("WorkGroupUniformLoadResult {ty:?}")
        }
        Expression::ArrayLength(handle) => format!("ArrayLength {handle:?}"),
        Expression::RayQueryProceedResult => "RayQueryProceedResult".to_string(),
        Expression::RayQueryGetIntersection { committed, .. } => {
            format!("RayQueryGetIntersection committed={committed}")
        }
        Expression::SubgroupBallotResult => "SubgroupBallotResult".to_string(),
        Expression::SubgroupOperationResult { ty } => format!("SubgroupOperationResult {ty:?}"),
    }
}

pub(super) fn expression_children(
    expr: &Expression,
    expressions: &Arena<Expression>,
    source: &str,
    depth: usize,
) -> Vec<PassDebugAstNode> {
    const MAX_EXPR_DEPTH: usize = 8;
    if depth >= MAX_EXPR_DEPTH {
        return vec![PassDebugAstNode::leaf("...")];
    }

    match expr {
        Expression::Compose { components, .. } => components
            .iter()
            .enumerate()
            .map(|(index, component)| {
                PassDebugAstNode::branch(
                    format!("component {index}"),
                    vec![expression_node(*component, expressions, source, depth + 1)],
                )
            })
            .collect(),
        Expression::Access { base, index } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[("base", Some(*base)), ("index", Some(*index))],
        ),
        Expression::AccessIndex { base, .. } => {
            expr_list_nodes(expressions, source, depth, &[("base", Some(*base))])
        }
        Expression::Splat { value, .. } => {
            expr_list_nodes(expressions, source, depth, &[("value", Some(*value))])
        }
        Expression::Swizzle { vector, .. } => {
            expr_list_nodes(expressions, source, depth, &[("vector", Some(*vector))])
        }
        Expression::Load { pointer } => {
            expr_list_nodes(expressions, source, depth, &[("pointer", Some(*pointer))])
        }
        Expression::ImageSample {
            image,
            sampler,
            coordinate,
            array_index,
            offset,
            depth_ref,
            ..
        } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[
                ("image", Some(*image)),
                ("sampler", Some(*sampler)),
                ("coordinate", Some(*coordinate)),
                ("array_index", *array_index),
                ("offset", *offset),
                ("depth_ref", *depth_ref),
            ],
        ),
        Expression::ImageLoad {
            image,
            coordinate,
            array_index,
            sample,
            level,
        } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[
                ("image", Some(*image)),
                ("coordinate", Some(*coordinate)),
                ("array_index", *array_index),
                ("sample", *sample),
                ("level", *level),
            ],
        ),
        Expression::ImageQuery { image, .. } => {
            expr_list_nodes(expressions, source, depth, &[("image", Some(*image))])
        }
        Expression::Unary { expr, .. } => {
            expr_list_nodes(expressions, source, depth, &[("expr", Some(*expr))])
        }
        Expression::Binary { left, right, .. } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[("left", Some(*left)), ("right", Some(*right))],
        ),
        Expression::Select {
            condition,
            accept,
            reject,
        } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[
                ("condition", Some(*condition)),
                ("accept", Some(*accept)),
                ("reject", Some(*reject)),
            ],
        ),
        Expression::Derivative { expr, .. } => {
            expr_list_nodes(expressions, source, depth, &[("expr", Some(*expr))])
        }
        Expression::Relational { argument, .. } => {
            expr_list_nodes(expressions, source, depth, &[("argument", Some(*argument))])
        }
        Expression::Math {
            arg,
            arg1,
            arg2,
            arg3,
            ..
        } => expr_list_nodes(
            expressions,
            source,
            depth,
            &[
                ("arg", Some(*arg)),
                ("arg1", *arg1),
                ("arg2", *arg2),
                ("arg3", *arg3),
            ],
        ),
        Expression::As { expr, .. } => {
            expr_list_nodes(expressions, source, depth, &[("expr", Some(*expr))])
        }
        Expression::RayQueryGetIntersection { query, .. } => {
            expr_list_nodes(expressions, source, depth, &[("query", Some(*query))])
        }
        other => vec![PassDebugAstNode::leaf(format!("{other:#?}"))],
    }
}

pub(super) fn expr_list_nodes(
    expressions: &Arena<Expression>,
    source: &str,
    depth: usize,
    handles: &[(&str, Option<naga::Handle<Expression>>)],
) -> Vec<PassDebugAstNode> {
    handles
        .iter()
        .filter_map(|(label, handle)| {
            handle.map(|handle| {
                PassDebugAstNode::branch(
                    *label,
                    vec![expression_node(handle, expressions, source, depth + 1)],
                )
            })
        })
        .collect()
}
