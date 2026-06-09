//! Render-pass shader debug documents.
//!
//! This module keeps the UI-facing representation intentionally small: the
//! original combined WGSL module plus collapsible trees built from Naga's IR.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use naga::{
    Arena, Block, Expression, Function, Module, ShaderStage, Statement, SwizzleComponent, Type,
    TypeInner,
};
use serde::Serialize;

const MAX_DEPENDENCY_DEPTH: usize = 48;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugSourceRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugAstNode {
    pub label: String,
    pub target_id: Option<String>,
    pub role: Option<String>,
    pub source_range: Option<PassDebugSourceRange>,
    pub children: Vec<PassDebugAstNode>,
}

impl PassDebugAstNode {
    fn leaf(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            target_id: None,
            role: None,
            source_range: None,
            children: Vec::new(),
        }
    }

    fn branch(label: impl Into<String>, children: Vec<PassDebugAstNode>) -> Self {
        Self {
            label: label.into(),
            target_id: None,
            role: None,
            source_range: None,
            children,
        }
    }

    fn with_source_range(mut self, source_range: Option<PassDebugSourceRange>) -> Self {
        self.source_range = source_range;
        self
    }

    fn with_target_range(
        mut self,
        target_id: impl Into<String>,
        role: impl Into<String>,
        source_range: Option<PassDebugSourceRange>,
    ) -> Self {
        self.target_id = Some(target_id.into());
        self.role = Some(role.into());
        self.source_range = source_range;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugDependencyTarget {
    pub id: String,
    pub name: String,
    pub label: String,
    pub scope: String,
    pub kind: String,
    pub source_range: Option<PassDebugSourceRange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugDependencyNode {
    pub label: String,
    pub edge_label: Option<String>,
    pub display_label: Option<String>,
    pub source_range: Option<PassDebugSourceRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_source_range: Option<PassDebugSourceRange>,
    pub target_id: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub reference: bool,
    pub children: Vec<PassDebugDependencyNode>,
}

impl PassDebugDependencyNode {
    fn leaf(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            edge_label: None,
            display_label: None,
            source_range: None,
            definition_source_range: None,
            target_id: None,
            reference: false,
            children: Vec::new(),
        }
    }

    fn branch(label: impl Into<String>, children: Vec<PassDebugDependencyNode>) -> Self {
        Self {
            label: label.into(),
            edge_label: None,
            display_label: None,
            source_range: None,
            definition_source_range: None,
            target_id: None,
            reference: false,
            children,
        }
    }

    fn target(
        label: impl Into<String>,
        target_id: impl Into<String>,
        children: Vec<PassDebugDependencyNode>,
    ) -> Self {
        Self {
            label: label.into(),
            edge_label: None,
            display_label: None,
            source_range: None,
            definition_source_range: None,
            target_id: Some(target_id.into()),
            reference: false,
            children,
        }
    }

    fn target_reference(label: impl Into<String>, target_id: impl Into<String>) -> Self {
        Self::target(label, target_id, Vec::new()).with_reference()
    }

    fn with_reference(mut self) -> Self {
        self.reference = true;
        self
    }

    fn with_edge_label(mut self, edge_label: Option<String>) -> Self {
        self.edge_label = edge_label;
        self
    }

    fn with_display_label(mut self, display_label: Option<String>) -> Self {
        self.display_label = display_label;
        self
    }

    fn with_source_range(mut self, source_range: Option<PassDebugSourceRange>) -> Self {
        self.source_range = source_range;
        self
    }

    fn with_definition_source_range(
        mut self,
        definition_source_range: Option<PassDebugSourceRange>,
    ) -> Self {
        self.definition_source_range = definition_source_range;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugSource {
    pub pass_name: String,
    pub module_source: String,
    pub ast_tree: Vec<PassDebugAstNode>,
    pub dependency_targets: Vec<PassDebugDependencyTarget>,
    pub dependency_trees: HashMap<String, PassDebugDependencyNode>,
    pub dependency_root_target_id: Option<String>,
    pub dependency_error: Option<String>,
    pub parse_error: Option<String>,
}

impl PassDebugSource {
    pub fn from_wgsl(pass_name: impl Into<String>, module_source: impl Into<String>) -> Self {
        let pass_name = pass_name.into();
        let module_source = module_source.into();
        match naga::front::wgsl::parse_str(&module_source) {
            Ok(module) => {
                let dependencies = build_dependency_debug(&module, &module_source);
                let ast_tree = module_to_ast_tree(&module, &module_source, &dependencies.targets);
                Self {
                    pass_name,
                    module_source,
                    ast_tree,
                    dependency_targets: dependencies.targets,
                    dependency_trees: dependencies.trees,
                    dependency_root_target_id: dependencies.root_target_id,
                    dependency_error: dependencies.error,
                    parse_error: None,
                }
            }
            Err(error) => Self {
                pass_name,
                module_source,
                ast_tree: vec![PassDebugAstNode::branch(
                    "Parse Error",
                    vec![PassDebugAstNode::leaf(error.to_string())],
                )],
                dependency_targets: Vec::new(),
                dependency_trees: HashMap::new(),
                dependency_root_target_id: None,
                dependency_error: None,
                parse_error: Some(error.to_string()),
            },
        }
    }
}

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

fn function_scope_for_handle(handle: naga::Handle<Function>, function: &Function) -> String {
    function
        .name
        .clone()
        .unwrap_or_else(|| format!("function_{}", handle.index()))
}

fn target_id_global(handle: naga::Handle<naga::GlobalVariable>) -> String {
    format!("global::{}", handle.index())
}

fn target_id_arg(scope: &str, index: u32) -> String {
    format!("{scope}::arg::{index}")
}

fn target_id_local(scope: &str, handle: naga::Handle<naga::LocalVariable>) -> String {
    format!("{scope}::local::{}", handle.index())
}

fn target_id_expr(scope: &str, handle: naga::Handle<Expression>) -> String {
    format!("{scope}::expr::{}", handle.index())
}

fn target_id_return(scope: &str) -> String {
    format!("{scope}::return")
}

fn entry_points_node(
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

fn functions_node(
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

fn globals_node(module: &Module, targets: &[PassDebugDependencyTarget]) -> PassDebugAstNode {
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

fn types_and_constants_node(module: &Module, source: &str) -> PassDebugAstNode {
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

fn function_children(
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

fn expressions_group_node(
    label: &str,
    expressions: &Arena<Expression>,
    source: &str,
) -> PassDebugAstNode {
    expressions_group_node_inner(label, expressions, source, None, &HashSet::new(), &[])
}

fn expressions_group_node_with_named(
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

fn expressions_group_node_inner(
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

fn block_to_nodes(
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

fn statement_kind_label(stmt: &Statement) -> String {
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

fn statement_children(
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

fn expression_node(
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

fn expression_label(handle: naga::Handle<Expression>, expr: &Expression) -> String {
    format!("{handle:?}: {}", expression_kind_label(expr))
}

fn expression_kind_label(expr: &Expression) -> String {
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

fn expression_children(
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

fn expr_list_nodes(
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

#[derive(Default)]
struct DependencyDebugBuild {
    targets: Vec<PassDebugDependencyTarget>,
    trees: HashMap<String, PassDebugDependencyNode>,
    root_target_id: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum ExprRef {
    Global(naga::Handle<Expression>),
    Function {
        scope: String,
        handle: naga::Handle<Expression>,
    },
}

impl ExprRef {
    fn key(&self) -> String {
        match self {
            Self::Global(handle) => format!("global:{}", handle.index()),
            Self::Function { scope, handle } => format!("{scope}:{}", handle.index()),
        }
    }
}

#[derive(Clone, Debug)]
enum TargetKind {
    Global(naga::Handle<naga::GlobalVariable>),
    Argument {
        scope: String,
        index: u32,
    },
    Local {
        scope: String,
        handle: naga::Handle<naga::LocalVariable>,
    },
    NamedExpression {
        scope: String,
        handle: naga::Handle<Expression>,
    },
    Return {
        scope: String,
    },
}

type DefinitionId = usize;
type DefinitionEnv = HashMap<String, Vec<DefinitionId>>;

#[derive(Clone, Debug)]
struct ControlDependency {
    label: String,
    expr: ExprRef,
    env: DefinitionEnv,
}

#[derive(Clone, Debug)]
struct DefinitionSite {
    target_id: String,
    value: Option<ExprRef>,
    controls: Vec<ControlDependency>,
    env: DefinitionEnv,
    source_range: Option<PassDebugSourceRange>,
}

#[derive(Clone, Debug)]
struct ReturnDependency {
    value: Option<naga::Handle<Expression>>,
    controls: Vec<ControlDependency>,
    env: DefinitionEnv,
}

#[derive(Clone, Debug)]
struct CallDependency {
    function: naga::Handle<Function>,
    arguments: Vec<naga::Handle<Expression>>,
}

#[derive(Clone, Debug)]
struct AccessPathTarget {
    target_id: String,
    display_label: String,
    source_range: Option<PassDebugSourceRange>,
    projection: Option<AccessProjection>,
}

type AccessProjection = Vec<u32>;

struct DependencyAnalyzer<'a> {
    module: &'a Module,
    source: &'a str,
    functions: HashMap<String, &'a Function>,
    function_handles: HashMap<naga::Handle<Function>, String>,
    targets: Vec<PassDebugDependencyTarget>,
    target_kinds: HashMap<String, TargetKind>,
    definitions: Vec<DefinitionSite>,
    final_definitions_by_target: HashMap<String, Vec<DefinitionId>>,
    expression_envs: HashMap<String, DefinitionEnv>,
    returns_by_function: HashMap<String, Vec<ReturnDependency>>,
    calls_by_result: HashMap<(String, usize), CallDependency>,
    store_lhs_counts_by_target: HashMap<String, usize>,
    entry_scopes: Vec<String>,
    fragment_entry_scopes: Vec<String>,
}

impl<'a> DependencyAnalyzer<'a> {
    fn new(module: &'a Module, source: &'a str) -> Self {
        let mut analyzer = Self {
            module,
            source,
            functions: HashMap::new(),
            function_handles: HashMap::new(),
            targets: Vec::new(),
            target_kinds: HashMap::new(),
            definitions: Vec::new(),
            final_definitions_by_target: HashMap::new(),
            expression_envs: HashMap::new(),
            returns_by_function: HashMap::new(),
            calls_by_result: HashMap::new(),
            store_lhs_counts_by_target: HashMap::new(),
            entry_scopes: Vec::new(),
            fragment_entry_scopes: Vec::new(),
        };
        analyzer.index_module();
        analyzer
    }

    fn into_debug(mut self) -> DependencyDebugBuild {
        self.add_entry_return_targets();
        let root_target_id = self.root_target_id();
        self.targets
            .sort_by(|a, b| a.scope.cmp(&b.scope).then_with(|| a.name.cmp(&b.name)));
        let mut trees = HashMap::new();
        for target in &self.targets {
            let tree = self.build_target_tree(&target.id, &mut Vec::new(), 0);
            trees.insert(target.id.clone(), tree);
        }
        DependencyDebugBuild {
            targets: self.targets,
            trees,
            root_target_id,
            error: None,
        }
    }

    fn index_module(&mut self) {
        for (handle, global) in self.module.global_variables.iter() {
            let name = global
                .name
                .clone()
                .unwrap_or_else(|| format!("global_{}", handle.index()));
            let id = target_id_global(handle);
            self.add_target(
                id,
                name.clone(),
                format!("global {name}"),
                "module".to_string(),
                "global".to_string(),
                TargetKind::Global(handle),
            );
        }

        for entry in &self.module.entry_points {
            let scope = entry.name.clone();
            self.entry_scopes.push(scope.clone());
            if entry.stage == ShaderStage::Fragment {
                self.fragment_entry_scopes.push(scope.clone());
            }
            self.functions.insert(scope.clone(), &entry.function);
            self.index_function(scope, &entry.function);
        }

        for (handle, function) in self.module.functions.iter() {
            let scope = function_scope_for_handle(handle, function);
            self.function_handles.insert(handle, scope.clone());
            self.functions.insert(scope.clone(), function);
            self.index_function(scope, function);
        }
    }

    fn add_entry_return_targets(&mut self) {
        let scopes = self
            .fragment_entry_scopes
            .iter()
            .chain(self.entry_scopes.iter())
            .cloned()
            .collect::<Vec<_>>();
        for scope in scopes {
            if !self.returns_by_function.contains_key(&scope) {
                continue;
            }
            self.add_target(
                target_id_return(&scope),
                "return".to_string(),
                format!("{scope} return"),
                scope.clone(),
                "return".to_string(),
                TargetKind::Return { scope },
            );
        }
    }

    fn root_target_id(&self) -> Option<String> {
        self.fragment_entry_scopes
            .iter()
            .map(|scope| target_id_return(scope))
            .find(|target_id| self.target_kinds.contains_key(target_id))
            .or_else(|| {
                self.entry_scopes
                    .iter()
                    .map(|scope| target_id_return(scope))
                    .find(|target_id| self.target_kinds.contains_key(target_id))
            })
            .or_else(|| self.targets.first().map(|target| target.id.clone()))
    }

    fn index_function(&mut self, scope: String, function: &'a Function) {
        for (index, arg) in function.arguments.iter().enumerate() {
            let name = arg.name.clone().unwrap_or_else(|| format!("arg_{index}"));
            self.add_target(
                target_id_arg(&scope, index as u32),
                name.clone(),
                format!("{scope} argument {name}"),
                scope.clone(),
                "argument".to_string(),
                TargetKind::Argument {
                    scope: scope.clone(),
                    index: index as u32,
                },
            );
        }

        for (handle, local) in function.local_variables.iter() {
            let name = local
                .name
                .clone()
                .unwrap_or_else(|| format!("local_{}", handle.index()));
            self.add_target(
                target_id_local(&scope, handle),
                name.clone(),
                format!("{scope} local {name}"),
                scope.clone(),
                "local".to_string(),
                TargetKind::Local {
                    scope: scope.clone(),
                    handle,
                },
            );
        }

        for (handle, name) in function.named_expressions.iter() {
            self.add_target(
                target_id_expr(&scope, *handle),
                name.clone(),
                format!("{scope} let {name}"),
                scope.clone(),
                "let".to_string(),
                TargetKind::NamedExpression {
                    scope: scope.clone(),
                    handle: *handle,
                },
            );
        }

        let mut env = self.initial_function_definition_env(&scope, function);
        self.collect_block_dependencies(
            &scope,
            function,
            &function.body,
            &mut Vec::new(),
            &mut env,
        );
        self.merge_final_definitions(env);
    }

    fn add_target(
        &mut self,
        id: String,
        name: String,
        label: String,
        scope: String,
        kind: String,
        target_kind: TargetKind,
    ) {
        if self.target_kinds.contains_key(&id) {
            return;
        }
        let source_range = self.source_range_for_target(&target_kind, &name);
        self.target_kinds.insert(id.clone(), target_kind);
        self.targets.push(PassDebugDependencyTarget {
            id,
            name,
            label,
            scope,
            kind,
            source_range,
        });
    }

    fn source_range_for_target(
        &self,
        target_kind: &TargetKind,
        name: &str,
    ) -> Option<PassDebugSourceRange> {
        match target_kind {
            TargetKind::Global(handle) => self
                .module
                .global_variables
                .get_span(*handle)
                .to_range()
                .and_then(|range| find_identifier_range(self.source, range, name))
                .or_else(|| find_global_identifier_range(self.source, name)),
            TargetKind::Argument { scope, .. } => {
                find_argument_identifier_range(self.source, scope, name)
            }
            TargetKind::Local { scope, handle } => self
                .functions
                .get(scope)
                .and_then(|function| function.local_variables.get_span(*handle).to_range())
                .and_then(|range| find_identifier_range(self.source, range, name))
                .or_else(|| find_keyword_identifier_in_scope(self.source, scope, "var", name)),
            TargetKind::NamedExpression { scope, handle } => {
                find_keyword_identifier_in_scope(self.source, scope, "let", name).or_else(|| {
                    self.functions
                        .get(scope)
                        .and_then(|function| function.expressions.get_span(*handle).to_range())
                        .and_then(|range| source_range_from_byte_range(self.source, range))
                })
            }
            TargetKind::Return { .. } => None,
        }
    }

    fn initial_function_definition_env(
        &mut self,
        scope: &str,
        function: &'a Function,
    ) -> DefinitionEnv {
        let mut env = DefinitionEnv::new();
        for (handle, local) in function.local_variables.iter() {
            let Some(init) = local.init else {
                continue;
            };
            let target_id = target_id_local(scope, handle);
            let definition_id = self.add_definition(DefinitionSite {
                target_id: target_id.clone(),
                value: Some(ExprRef::Function {
                    scope: scope.to_string(),
                    handle: init,
                }),
                controls: Vec::new(),
                env: env.clone(),
                source_range: target_source_range(&self.targets, &target_id),
            });
            env.insert(target_id, vec![definition_id]);
        }
        env
    }

    fn add_definition(&mut self, definition: DefinitionSite) -> DefinitionId {
        let definition_id = self.definitions.len();
        self.definitions.push(definition);
        definition_id
    }

    fn merge_final_definitions(&mut self, env: DefinitionEnv) {
        merge_definition_env_into(&mut self.final_definitions_by_target, env);
    }

    fn collect_block_dependencies(
        &mut self,
        scope: &str,
        function: &'a Function,
        block: &Block,
        controls: &mut Vec<ControlDependency>,
        env: &mut DefinitionEnv,
    ) {
        for stmt in block {
            match stmt {
                Statement::Emit(range) => {
                    for handle in range.clone() {
                        let expr_ref = ExprRef::Function {
                            scope: scope.to_string(),
                            handle,
                        };
                        self.expression_envs
                            .entry(expr_ref.key())
                            .or_insert_with(|| env.clone());
                    }
                }
                Statement::Block(block) => {
                    self.collect_block_dependencies(scope, function, block, controls, env);
                }
                Statement::If {
                    condition,
                    accept,
                    reject,
                } => {
                    let before = env.clone();
                    controls.push(ControlDependency {
                        label: "[condition] if".to_string(),
                        expr: ExprRef::Function {
                            scope: scope.to_string(),
                            handle: *condition,
                        },
                        env: before.clone(),
                    });
                    let mut accept_env = before.clone();
                    self.collect_block_dependencies(
                        scope,
                        function,
                        accept,
                        controls,
                        &mut accept_env,
                    );
                    let mut reject_env = before;
                    self.collect_block_dependencies(
                        scope,
                        function,
                        reject,
                        controls,
                        &mut reject_env,
                    );
                    *env = merge_definition_envs([accept_env, reject_env]);
                    controls.pop();
                }
                Statement::Switch { selector, cases } => {
                    let before = env.clone();
                    controls.push(ControlDependency {
                        label: "[condition] switch selector".to_string(),
                        expr: ExprRef::Function {
                            scope: scope.to_string(),
                            handle: *selector,
                        },
                        env: before.clone(),
                    });
                    let mut merged_env = before.clone();
                    for case in cases {
                        controls.push(ControlDependency {
                            label: format!("[condition] case {:?}", case.value),
                            expr: ExprRef::Function {
                                scope: scope.to_string(),
                                handle: *selector,
                            },
                            env: before.clone(),
                        });
                        let mut case_env = before.clone();
                        self.collect_block_dependencies(
                            scope,
                            function,
                            &case.body,
                            controls,
                            &mut case_env,
                        );
                        merge_definition_env_into(&mut merged_env, case_env);
                        controls.pop();
                    }
                    *env = merged_env;
                    controls.pop();
                }
                Statement::Loop {
                    body,
                    continuing,
                    break_if,
                } => {
                    let before = env.clone();
                    if let Some(expr) = break_if {
                        controls.push(ControlDependency {
                            label: "[condition] loop break_if".to_string(),
                            expr: ExprRef::Function {
                                scope: scope.to_string(),
                                handle: *expr,
                            },
                            env: before.clone(),
                        });
                    }
                    let mut loop_env = before.clone();
                    self.collect_block_dependencies(scope, function, body, controls, &mut loop_env);
                    self.collect_block_dependencies(
                        scope,
                        function,
                        continuing,
                        controls,
                        &mut loop_env,
                    );
                    *env = merge_definition_envs([before, loop_env]);
                    if break_if.is_some() {
                        controls.pop();
                    }
                }
                Statement::Store { pointer, value } => {
                    let pointer_ref = ExprRef::Function {
                        scope: scope.to_string(),
                        handle: *pointer,
                    };
                    let value_ref = ExprRef::Function {
                        scope: scope.to_string(),
                        handle: *value,
                    };
                    self.expression_envs.insert(value_ref.key(), env.clone());
                    if let Some(target_id) = self.resolve_pointer_target(&pointer_ref) {
                        let fallback_source_range =
                            self.next_store_lhs_source_range(scope, &target_id);
                        let source_range = self
                            .store_lhs_source_range(
                                scope,
                                &pointer_ref,
                                &ExprRef::Function {
                                    scope: scope.to_string(),
                                    handle: *value,
                                },
                                &target_id,
                            )
                            .or(fallback_source_range);
                        let definition_id = self.add_definition(DefinitionSite {
                            target_id: target_id.clone(),
                            value: Some(value_ref),
                            controls: controls.clone(),
                            env: env.clone(),
                            source_range,
                        });
                        env.insert(target_id, vec![definition_id]);
                    }
                }
                Statement::Return { value } => {
                    self.returns_by_function
                        .entry(scope.to_string())
                        .or_default()
                        .push(ReturnDependency {
                            value: *value,
                            controls: controls.clone(),
                            env: env.clone(),
                        });
                }
                Statement::Call {
                    function,
                    arguments,
                    result,
                } => {
                    if let Some(result) = result {
                        let result_ref = ExprRef::Function {
                            scope: scope.to_string(),
                            handle: *result,
                        };
                        self.expression_envs.insert(result_ref.key(), env.clone());
                        self.calls_by_result.insert(
                            (scope.to_string(), result.index()),
                            CallDependency {
                                function: *function,
                                arguments: arguments.clone(),
                            },
                        );
                    }
                }
                _ => {}
            }
        }
    }

    fn build_target_tree(
        &self,
        target_id: &str,
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> PassDebugDependencyNode {
        self.build_target_tree_with_edge(target_id, target_stack, depth, None)
    }

    fn build_target_tree_with_edge(
        &self,
        target_id: &str,
        target_stack: &mut Vec<String>,
        depth: usize,
        edge_label: Option<String>,
    ) -> PassDebugDependencyNode {
        self.build_target_tree_with_context(target_id, target_stack, depth, edge_label, None, None)
    }

    fn build_target_tree_with_context(
        &self,
        target_id: &str,
        target_stack: &mut Vec<String>,
        depth: usize,
        edge_label: Option<String>,
        display_label: Option<String>,
        source_range: Option<PassDebugSourceRange>,
    ) -> PassDebugDependencyNode {
        let target = self.targets.iter().find(|target| target.id == target_id);
        if self.should_reference_target(target_id, target_stack)
            && let Some(reference) = self.target_reference_node(
                target_id,
                edge_label.clone(),
                display_label.clone(),
                source_range,
                None,
            )
        {
            return reference;
        }
        if depth >= MAX_DEPENDENCY_DEPTH {
            if let Some(reference) =
                self.target_reference_node(target_id, edge_label, display_label, source_range, None)
            {
                return reference;
            }
            return PassDebugDependencyNode::target(
                format!("{target_id} [depth limit]"),
                target_id.to_string(),
                Vec::new(),
            );
        }
        if target_stack.iter().any(|id| id == target_id) {
            return PassDebugDependencyNode::target(
                format!("{target_id} [cycle]"),
                target_id.to_string(),
                Vec::new(),
            )
            .with_edge_label(edge_label)
            .with_display_label(display_label);
        }

        let Some(target) = target else {
            return PassDebugDependencyNode::leaf(format!("missing target {target_id}"));
        };
        let Some(kind) = self.target_kinds.get(target_id) else {
            return PassDebugDependencyNode::leaf(format!("missing target kind {target_id}"));
        };

        target_stack.push(target_id.to_string());
        let mut children = match kind {
            TargetKind::Global(handle) => self.global_target_children(*handle, target_stack, depth),
            TargetKind::Argument { scope, index } => {
                vec![PassDebugDependencyNode::leaf(format!(
                    "[source] function argument {scope}::{index}"
                ))]
            }
            TargetKind::Local { scope, handle } => {
                self.local_target_children(scope, *handle, target_id, target_stack, depth)
            }
            TargetKind::NamedExpression { scope, handle } => self.semantic_expr_dependencies(
                ExprRef::Function {
                    scope: scope.clone(),
                    handle: *handle,
                },
                None,
                None,
                target_stack,
                &mut HashSet::new(),
                &mut Vec::new(),
                depth + 1,
            ),
            TargetKind::Return { scope } => {
                self.return_target_children(scope, target_stack, depth + 1)
            }
        };
        if children.is_empty() {
            children.push(PassDebugDependencyNode::leaf("[source] no contributors"));
        }
        target_stack.pop();

        PassDebugDependencyNode::target(
            dependency_target_node_label(target, display_label.as_deref()),
            target.id.clone(),
            children,
        )
        .with_edge_label(edge_label)
        .with_display_label(display_label)
        .with_source_range(source_range)
    }

    fn global_target_children(
        &self,
        handle: naga::Handle<naga::GlobalVariable>,
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        let mut children = Vec::new();
        let global = &self.module.global_variables[handle];
        if let Some(init) = global.init {
            children.extend(self.semantic_expr_dependencies(
                ExprRef::Global(init),
                None,
                None,
                target_stack,
                &mut HashSet::new(),
                &mut Vec::new(),
                depth + 1,
            ));
        }
        let target_id = target_id_global(handle);
        children.extend(self.final_definition_nodes_for_target(
            &target_id,
            target_stack,
            depth + 1,
        ));
        children
    }

    fn local_target_children(
        &self,
        scope: &str,
        handle: naga::Handle<naga::LocalVariable>,
        target_id: &str,
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        let definitions =
            self.final_definition_nodes_for_target(target_id, target_stack, depth + 1);
        if !definitions.is_empty() {
            return definitions;
        }

        let mut children = Vec::new();
        if let Some(function) = self.functions.get(scope)
            && let Some(init) = function.local_variables[handle].init
        {
            children.extend(self.semantic_expr_dependencies(
                ExprRef::Function {
                    scope: scope.to_string(),
                    handle: init,
                },
                None,
                None,
                target_stack,
                &mut HashSet::new(),
                &mut Vec::new(),
                depth + 1,
            ));
        }
        children
    }

    fn final_definition_nodes_for_target(
        &self,
        target_id: &str,
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        self.final_definitions_by_target
            .get(target_id)
            .map(|definitions| {
                definitions
                    .iter()
                    .map(|definition_id| {
                        self.build_definition_node(
                            *definition_id,
                            None,
                            None,
                            None,
                            None,
                            true,
                            target_stack,
                            &mut HashSet::new(),
                            &mut Vec::new(),
                            depth + 1,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn definition_nodes_for_target(
        &self,
        target_id: &str,
        edge_label: Option<String>,
        display_label: Option<String>,
        source_range: Option<PassDebugSourceRange>,
        projection: Option<&AccessProjection>,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Option<Vec<PassDebugDependencyNode>> {
        let definitions = env?.get(target_id)?;
        Some(
            definitions
                .iter()
                .map(|definition_id| {
                    let depth = if self.is_root_target(target_id, target_stack) {
                        1
                    } else {
                        depth + 1
                    };
                    if self.should_reference_target(target_id, target_stack)
                        && let Some(reference) = self.build_definition_reference_node(
                            *definition_id,
                            edge_label.clone(),
                            display_label.clone(),
                            source_range,
                        )
                    {
                        return reference;
                    }
                    self.build_definition_node(
                        *definition_id,
                        edge_label.clone(),
                        display_label.clone(),
                        source_range,
                        projection,
                        false,
                        target_stack,
                        seen_exprs,
                        definition_stack,
                        depth,
                    )
                })
                .collect(),
        )
    }

    fn build_definition_node(
        &self,
        definition_id: DefinitionId,
        edge_label: Option<String>,
        display_label: Option<String>,
        source_range: Option<PassDebugSourceRange>,
        projection: Option<&AccessProjection>,
        definition_is_occurrence: bool,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> PassDebugDependencyNode {
        let Some(definition) = self.definitions.get(definition_id) else {
            return PassDebugDependencyNode::leaf(format!("missing definition {definition_id}"));
        };
        let Some(target) = self
            .targets
            .iter()
            .find(|target| target.id == definition.target_id)
        else {
            return PassDebugDependencyNode::leaf(format!(
                "missing target {}",
                definition.target_id
            ));
        };
        let node_source_range = if definition_is_occurrence {
            source_range.or(definition.source_range)
        } else {
            source_range
        };
        if self.should_reference_target(&definition.target_id, target_stack) {
            let display_label =
                self.definition_display_label(target, definition, display_label, node_source_range);
            return self
                .target_reference_node(
                    &definition.target_id,
                    edge_label,
                    display_label,
                    node_source_range,
                    definition.source_range,
                )
                .unwrap_or_else(|| {
                    PassDebugDependencyNode::target(
                        target.label.clone(),
                        target.id.clone(),
                        Vec::new(),
                    )
                });
        }
        if depth >= MAX_DEPENDENCY_DEPTH {
            let definition_source_range =
                distinct_definition_source_range(definition.source_range, node_source_range);
            let display_label =
                self.definition_display_label(target, definition, display_label, node_source_range);
            return self
                .target_reference_node(
                    &target.id,
                    edge_label.clone(),
                    display_label.clone(),
                    node_source_range,
                    definition.source_range,
                )
                .map(|node| node.with_definition_source_range(definition_source_range))
                .unwrap_or_else(|| {
                    PassDebugDependencyNode::target(
                        format!("{} [depth limit]", target.label),
                        target.id.clone(),
                        Vec::new(),
                    )
                    .with_edge_label(edge_label)
                    .with_display_label(display_label)
                    .with_source_range(node_source_range)
                    .with_definition_source_range(definition_source_range)
                });
        }
        if definition_stack.contains(&definition_id) {
            let definition_source_range =
                distinct_definition_source_range(definition.source_range, node_source_range);
            return PassDebugDependencyNode::target(
                format!("{} [cycle]", target.label),
                target.id.clone(),
                Vec::new(),
            )
            .with_edge_label(edge_label)
            .with_display_label(display_label)
            .with_source_range(node_source_range)
            .with_definition_source_range(definition_source_range);
        }

        definition_stack.push(definition_id);
        let mut children = self.control_nodes(&definition.controls, target_stack, depth + 1);
        if let Some(value) = definition.value.clone() {
            let value_occurrence_range = definition.source_range.and_then(|source_range| {
                self.definition_value_occurrence_range(source_range, &value)
            });
            if let Some(projection) = projection.filter(|projection| !projection.is_empty()) {
                children.extend(self.semantic_expr_dependencies_projected(
                    value,
                    None,
                    value_occurrence_range,
                    projection,
                    Some(&definition.env),
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ));
            } else {
                children.extend(self.semantic_expr_dependencies_with_hint(
                    value,
                    None,
                    value_occurrence_range,
                    Some(&definition.env),
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ));
            }
        }
        if children.is_empty() {
            children.push(PassDebugDependencyNode::leaf("[source] no contributors"));
        }
        definition_stack.pop();

        let definition_source_range =
            distinct_definition_source_range(definition.source_range, node_source_range);
        let display_label =
            self.definition_display_label(target, definition, display_label, node_source_range);
        PassDebugDependencyNode::target(
            dependency_target_node_label(target, display_label.as_deref()),
            target.id.clone(),
            children,
        )
        .with_edge_label(edge_label)
        .with_display_label(display_label)
        .with_source_range(node_source_range)
        .with_definition_source_range(definition_source_range)
    }

    fn build_definition_reference_node(
        &self,
        definition_id: DefinitionId,
        edge_label: Option<String>,
        display_label: Option<String>,
        source_range: Option<PassDebugSourceRange>,
    ) -> Option<PassDebugDependencyNode> {
        let definition = self.definitions.get(definition_id)?;
        let target = self
            .targets
            .iter()
            .find(|target| target.id == definition.target_id)?;
        let display_label =
            self.definition_display_label(target, definition, display_label, source_range);
        self.target_reference_node(
            &definition.target_id,
            edge_label,
            display_label,
            source_range,
            definition.source_range,
        )
    }

    fn build_named_expression_tree_with_context(
        &self,
        target_id: &str,
        expr_ref: &ExprRef,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        depth: usize,
        edge_label: Option<String>,
        display_label: Option<String>,
        source_range: Option<PassDebugSourceRange>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
    ) -> PassDebugDependencyNode {
        let target = self.targets.iter().find(|target| target.id == target_id);
        if self.should_reference_target(target_id, target_stack)
            && let Some(reference) = self.target_reference_node(
                target_id,
                edge_label.clone(),
                display_label.clone(),
                source_range,
                None,
            )
        {
            return reference;
        }
        if depth >= MAX_DEPENDENCY_DEPTH {
            if let Some(reference) =
                self.target_reference_node(target_id, edge_label, display_label, source_range, None)
            {
                return reference;
            }
            return PassDebugDependencyNode::target(
                format!("{target_id} [depth limit]"),
                target_id.to_string(),
                Vec::new(),
            );
        }
        if target_stack.iter().any(|id| id == target_id) {
            return PassDebugDependencyNode::target(
                format!("{target_id} [cycle]"),
                target_id.to_string(),
                Vec::new(),
            )
            .with_edge_label(edge_label)
            .with_display_label(display_label)
            .with_source_range(source_range);
        }
        let Some(target) = target else {
            return PassDebugDependencyNode::leaf(format!("missing target {target_id}"));
        };

        target_stack.push(target_id.to_string());
        let mut children = self.semantic_expr_dependencies(
            expr_ref.clone(),
            None,
            env,
            target_stack,
            seen_exprs,
            definition_stack,
            depth + 1,
        );
        if children.is_empty() {
            children.push(PassDebugDependencyNode::leaf("[source] no contributors"));
        }
        target_stack.pop();

        PassDebugDependencyNode::target(
            dependency_target_node_label(target, display_label.as_deref()),
            target.id.clone(),
            children,
        )
        .with_edge_label(edge_label)
        .with_display_label(display_label)
        .with_source_range(source_range)
    }

    fn should_reference_target(&self, target_id: &str, target_stack: &[String]) -> bool {
        target_stack
            .first()
            .is_some_and(|root_target_id| root_target_id != target_id)
    }

    fn is_root_target(&self, target_id: &str, target_stack: &[String]) -> bool {
        target_stack
            .first()
            .is_some_and(|root_target_id| root_target_id == target_id)
    }

    fn target_reference_node(
        &self,
        target_id: &str,
        edge_label: Option<String>,
        display_label: Option<String>,
        source_range: Option<PassDebugSourceRange>,
        definition_source_range: Option<PassDebugSourceRange>,
    ) -> Option<PassDebugDependencyNode> {
        let target = self.targets.iter().find(|target| target.id == target_id)?;
        let node_source_range = source_range;
        let definition_source_range = distinct_definition_source_range(
            definition_source_range.or(target.source_range),
            node_source_range,
        );
        Some(
            PassDebugDependencyNode::target_reference(
                dependency_target_node_label(target, display_label.as_deref()),
                target.id.clone(),
            )
            .with_edge_label(edge_label)
            .with_display_label(display_label)
            .with_source_range(node_source_range)
            .with_definition_source_range(definition_source_range),
        )
    }

    fn definition_display_label(
        &self,
        target: &PassDebugDependencyTarget,
        definition: &DefinitionSite,
        display_label: Option<String>,
        source_range: Option<PassDebugSourceRange>,
    ) -> Option<String> {
        let base = display_label.unwrap_or_else(|| target.name.clone());
        let has_multiple_definitions = self
            .definitions
            .iter()
            .filter(|definition| definition.target_id == target.id)
            .take(2)
            .count()
            > 1;
        if has_multiple_definitions && let Some(range) = source_range.or(definition.source_range) {
            Some(format!("{base} ({})", range.line))
        } else {
            Some(base)
        }
    }

    fn return_target_children(
        &self,
        scope: &str,
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        let Some(returns) = self.returns_by_function.get(scope) else {
            return Vec::new();
        };
        if let [ret] = returns.as_slice()
            && ret.controls.is_empty()
        {
            return ret
                .value
                .map(|value| {
                    self.semantic_expr_dependencies(
                        ExprRef::Function {
                            scope: scope.to_string(),
                            handle: value,
                        },
                        None,
                        Some(&ret.env),
                        target_stack,
                        &mut HashSet::new(),
                        &mut Vec::new(),
                        depth + 1,
                    )
                })
                .unwrap_or_default();
        }

        returns
            .iter()
            .enumerate()
            .map(|(index, ret)| {
                let mut children = self.control_nodes(&ret.controls, target_stack, depth + 1);
                if let Some(value) = ret.value {
                    children.extend(self.semantic_expr_dependencies(
                        ExprRef::Function {
                            scope: scope.to_string(),
                            handle: value,
                        },
                        None,
                        Some(&ret.env),
                        target_stack,
                        &mut HashSet::new(),
                        &mut Vec::new(),
                        depth + 1,
                    ));
                }
                PassDebugDependencyNode::branch(format!("[return {index}] {scope}"), children)
            })
            .collect()
    }

    fn control_nodes(
        &self,
        controls: &[ControlDependency],
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        controls
            .iter()
            .map(|control| {
                self.semantic_relation_node(
                    control.expr.clone(),
                    control.label.as_str(),
                    Some(&control.env),
                    target_stack,
                    &mut HashSet::new(),
                    &mut Vec::new(),
                    depth + 1,
                )
            })
            .collect()
    }

    fn semantic_relation_node(
        &self,
        expr_ref: ExprRef,
        relation_label: &str,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> PassDebugDependencyNode {
        let children = self.semantic_expr_dependencies(
            expr_ref,
            None,
            env,
            target_stack,
            seen_exprs,
            definition_stack,
            depth + 1,
        );
        if children.is_empty() {
            PassDebugDependencyNode::leaf(format!("{relation_label} [no variable dependencies]"))
        } else {
            PassDebugDependencyNode::branch(relation_label.to_string(), children)
        }
    }

    fn semantic_expr_dependencies(
        &self,
        expr_ref: ExprRef,
        inherited_edge: Option<String>,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        self.semantic_expr_dependencies_with_hint(
            expr_ref,
            inherited_edge,
            None,
            env,
            target_stack,
            seen_exprs,
            definition_stack,
            depth,
        )
    }

    fn semantic_expr_dependencies_projected(
        &self,
        expr_ref: ExprRef,
        inherited_edge: Option<String>,
        occurrence_range: Option<PassDebugSourceRange>,
        projection: &AccessProjection,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        if projection.is_empty() {
            return Vec::new();
        }
        let effective_env = env.or_else(|| self.expression_envs.get(&expr_ref.key()));
        if let Some(access_path) = self.access_path_target(&expr_ref, target_stack) {
            let target_projection =
                combine_access_projection(access_path.projection.as_ref(), projection);
            if target_projection.is_empty() {
                return Vec::new();
            }
            let mut dependencies = self
                .definition_nodes_for_target(
                    &access_path.target_id,
                    inherited_edge.clone(),
                    Some(access_path.display_label.clone()),
                    occurrence_range.or(access_path.source_range),
                    Some(&target_projection),
                    effective_env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
                .unwrap_or_else(|| {
                    vec![self.build_target_tree_with_context(
                        &access_path.target_id,
                        target_stack,
                        depth + 1,
                        inherited_edge.clone(),
                        Some(access_path.display_label),
                        occurrence_range.or(access_path.source_range),
                    )]
                });
            dependencies.extend(self.access_path_index_dependencies(
                &expr_ref,
                inherited_edge,
                effective_env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ));
            return dependencies;
        }

        if self.named_expression_target_id(&expr_ref).is_some() {
            return self.semantic_expr_dependencies_with_hint(
                expr_ref,
                inherited_edge,
                occurrence_range,
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            );
        }

        if depth >= MAX_DEPENDENCY_DEPTH {
            return vec![PassDebugDependencyNode::leaf("[depth limit]")];
        }

        let expr_key = expr_ref.key();
        if !seen_exprs.insert(expr_key.clone()) {
            return Vec::new();
        }

        let Some(expr) = self.expression(&expr_ref) else {
            seen_exprs.remove(&expr_key);
            return Vec::new();
        };

        let children = match expr {
            Expression::Compose { components, .. } => self.projected_compose_dependencies(
                &expr_ref,
                components,
                operation_edge(inherited_edge, "Compose"),
                projection,
                effective_env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Splat { value, .. } => self.projected_operand_dependencies(
                &expr_ref,
                [(*value, None)],
                operation_edge(inherited_edge, "Splat"),
                effective_env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Binary { op, left, right } => self.projected_operand_dependencies(
                &expr_ref,
                [
                    (*left, self.operand_projection(&expr_ref, *left, projection)),
                    (
                        *right,
                        self.operand_projection(&expr_ref, *right, projection),
                    ),
                ],
                operation_edge(inherited_edge, format!("{op:?}")),
                effective_env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Unary { op, expr } => self.projected_operand_dependencies(
                &expr_ref,
                [(*expr, Some(projection.clone()))],
                operation_edge(inherited_edge, format!("{op:?}")),
                effective_env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::As { expr, .. } => self.projected_operand_dependencies(
                &expr_ref,
                [(*expr, Some(projection.clone()))],
                operation_edge(inherited_edge, "As"),
                effective_env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Derivative { axis, ctrl, expr } => self.projected_operand_dependencies(
                &expr_ref,
                [(*expr, Some(projection.clone()))],
                operation_edge(inherited_edge, format!("{axis:?}/{ctrl:?}")),
                effective_env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Math {
                fun,
                arg,
                arg1,
                arg2,
                arg3,
            } => {
                let mut operands =
                    vec![(*arg, self.operand_projection(&expr_ref, *arg, projection))];
                for handle in [*arg1, *arg2, *arg3].into_iter().flatten() {
                    operands.push((
                        handle,
                        self.operand_projection(&expr_ref, handle, projection),
                    ));
                }
                self.projected_operand_dependencies(
                    &expr_ref,
                    operands,
                    operation_edge(inherited_edge, format!("{fun:?}")),
                    effective_env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
            }
            Expression::Select {
                condition,
                accept,
                reject,
            } => {
                let mut dependencies = self.projected_operand_dependencies(
                    &expr_ref,
                    [(*condition, None)],
                    operation_edge(inherited_edge.clone(), "Select"),
                    effective_env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                );
                dependencies.extend(self.projected_operand_dependencies(
                    &expr_ref,
                    [
                        (
                            *accept,
                            self.operand_projection(&expr_ref, *accept, projection),
                        ),
                        (
                            *reject,
                            self.operand_projection(&expr_ref, *reject, projection),
                        ),
                    ],
                    operation_edge(inherited_edge, "Select"),
                    effective_env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ));
                dependencies
            }
            Expression::CallResult(_) => {
                let ExprRef::Function { scope, handle } = &expr_ref else {
                    seen_exprs.remove(&expr_key);
                    return Vec::new();
                };
                let Some(call) = self.calls_by_result.get(&(scope.clone(), handle.index())) else {
                    seen_exprs.remove(&expr_key);
                    return Vec::new();
                };
                let call_edge = self
                    .function_handles
                    .get(&call.function)
                    .cloned()
                    .unwrap_or_else(|| "call".to_string());
                let operands = call.arguments.iter().copied().map(|handle| {
                    (
                        handle,
                        self.operand_projection(&expr_ref, handle, projection),
                    )
                });
                self.projected_operand_dependencies(
                    &expr_ref,
                    operands,
                    operation_edge(inherited_edge, call_edge),
                    effective_env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
            }
            _ => self.semantic_expr_dependencies_with_hint(
                expr_ref.clone(),
                inherited_edge,
                occurrence_range,
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
        };

        seen_exprs.remove(&expr_key);
        children
    }

    fn semantic_expr_dependencies_with_hint(
        &self,
        expr_ref: ExprRef,
        inherited_edge: Option<String>,
        occurrence_range: Option<PassDebugSourceRange>,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        let effective_env = env.or_else(|| self.expression_envs.get(&expr_ref.key()));
        if let Some(target_id) = self.named_expression_target_id(&expr_ref)
            && !target_stack.iter().any(|id| id == &target_id)
        {
            return vec![self.build_named_expression_tree_with_context(
                &target_id,
                &expr_ref,
                effective_env,
                target_stack,
                depth + 1,
                inherited_edge,
                None,
                occurrence_range,
                seen_exprs,
                definition_stack,
            )];
        }

        if let Some(access_path) = self.access_path_target(&expr_ref, target_stack) {
            let mut dependencies = self
                .definition_nodes_for_target(
                    &access_path.target_id,
                    inherited_edge.clone(),
                    Some(access_path.display_label.clone()),
                    occurrence_range.or(access_path.source_range),
                    access_path.projection.as_ref(),
                    effective_env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
                .unwrap_or_else(|| {
                    vec![self.build_target_tree_with_context(
                        &access_path.target_id,
                        target_stack,
                        depth + 1,
                        inherited_edge.clone(),
                        Some(access_path.display_label),
                        occurrence_range.or(access_path.source_range),
                    )]
                });
            dependencies.extend(self.access_path_index_dependencies(
                &expr_ref,
                inherited_edge,
                effective_env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ));
            return dependencies;
        }

        if depth >= MAX_DEPENDENCY_DEPTH {
            return vec![PassDebugDependencyNode::leaf("[depth limit]")];
        }

        let expr_key = expr_ref.key();
        if !seen_exprs.insert(expr_key.clone()) {
            return Vec::new();
        }

        let Some(expr) = self.expression(&expr_ref) else {
            seen_exprs.remove(&expr_key);
            return Vec::new();
        };

        let children = self.semantic_expression_children(
            &expr_ref,
            expr,
            inherited_edge,
            effective_env,
            target_stack,
            seen_exprs,
            definition_stack,
            depth + 1,
        );
        seen_exprs.remove(&expr_key);
        children
    }

    fn semantic_expression_children(
        &self,
        expr_ref: &ExprRef,
        expr: &Expression,
        inherited_edge: Option<String>,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        match expr {
            Expression::Literal(_) | Expression::ZeroValue(_) => Vec::new(),
            Expression::Constant(handle) => {
                let init = self.module.constants[*handle].init;
                self.semantic_expr_dependencies(
                    ExprRef::Global(init),
                    inherited_edge,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
            }
            Expression::Override(handle) => self.module.overrides[*handle]
                .init
                .map(|init| {
                    self.semantic_expr_dependencies(
                        ExprRef::Global(init),
                        inherited_edge,
                        env,
                        target_stack,
                        seen_exprs,
                        definition_stack,
                        depth + 1,
                    )
                })
                .unwrap_or_default(),
            Expression::Compose { components, .. } => self.semantic_operand_dependencies(
                expr_ref,
                components.iter().copied().map(Some),
                operation_edge(inherited_edge, "Compose"),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Access { base, index } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*base), Some(*index)],
                operation_edge(inherited_edge, "Access"),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::AccessIndex { base, .. } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*base)],
                operation_edge(inherited_edge, "Access"),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Splat { value, .. } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*value)],
                operation_edge(inherited_edge, "Splat"),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Swizzle { vector, .. } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*vector)],
                operation_edge(inherited_edge, "Swizzle"),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::FunctionArgument(index) => {
                vec![self.build_target_tree_with_edge(
                    &target_id_arg(expr_ref_scope(expr_ref), *index),
                    target_stack,
                    depth + 1,
                    inherited_edge,
                )]
            }
            Expression::GlobalVariable(handle) => vec![self.build_target_tree_with_edge(
                &target_id_global(*handle),
                target_stack,
                depth + 1,
                inherited_edge,
            )],
            Expression::LocalVariable(handle) => vec![self.build_target_tree_with_edge(
                &target_id_local(expr_ref_scope(expr_ref), *handle),
                target_stack,
                depth + 1,
                inherited_edge,
            )],
            Expression::Load { pointer } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*pointer)],
                inherited_edge,
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::ImageSample {
                image,
                sampler,
                coordinate,
                array_index,
                offset,
                depth_ref,
                ..
            } => self.semantic_operand_dependencies(
                expr_ref,
                [
                    Some(*image),
                    Some(*sampler),
                    Some(*coordinate),
                    *array_index,
                    *offset,
                    *depth_ref,
                ],
                Some("textureSample".to_string()),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::ImageLoad {
                image,
                coordinate,
                array_index,
                sample,
                level,
            } => self.semantic_operand_dependencies(
                expr_ref,
                [
                    Some(*image),
                    Some(*coordinate),
                    *array_index,
                    *sample,
                    *level,
                ],
                Some("textureLoad".to_string()),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::ImageQuery { image, .. } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*image)],
                Some("textureQuery".to_string()),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Unary { op, expr } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*expr)],
                operation_edge(inherited_edge, format!("{op:?}")),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Binary { op, left, right } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*left), Some(*right)],
                operation_edge(inherited_edge, format!("{op:?}")),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Select {
                condition,
                accept,
                reject,
            } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*condition), Some(*accept), Some(*reject)],
                operation_edge(inherited_edge, "Select"),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Derivative { axis, ctrl, expr } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*expr)],
                operation_edge(inherited_edge, format!("{axis:?}/{ctrl:?}")),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Relational { fun, argument } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*argument)],
                operation_edge(inherited_edge, format!("{fun:?}")),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::Math {
                fun,
                arg,
                arg1,
                arg2,
                arg3,
            } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*arg), *arg1, *arg2, *arg3],
                operation_edge(inherited_edge, format!("{fun:?}")),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::As { expr, .. } => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*expr)],
                operation_edge(inherited_edge, "As"),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::CallResult(_) => {
                let ExprRef::Function { scope, handle } = expr_ref else {
                    return Vec::new();
                };
                let Some(call) = self.calls_by_result.get(&(scope.clone(), handle.index())) else {
                    return Vec::new();
                };
                let call_edge = self
                    .function_handles
                    .get(&call.function)
                    .cloned()
                    .unwrap_or_else(|| "call".to_string());
                self.semantic_operand_dependencies(
                    expr_ref,
                    call.arguments.iter().copied().map(Some),
                    Some(call_edge),
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
            }
            Expression::ArrayLength(handle) => self.semantic_operand_dependencies(
                expr_ref,
                [Some(*handle)],
                operation_edge(inherited_edge, "arrayLength"),
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ),
            Expression::RayQueryGetIntersection { query, .. } => self
                .semantic_operand_dependencies(
                    expr_ref,
                    [Some(*query)],
                    operation_edge(inherited_edge, "rayQueryGetIntersection"),
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ),
            _ => Vec::new(),
        }
    }

    fn semantic_operand_dependencies<I>(
        &self,
        current: &ExprRef,
        operands: I,
        edge_label: Option<String>,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode>
    where
        I: IntoIterator<Item = Option<naga::Handle<Expression>>>,
    {
        let search_ranges = self.operand_search_byte_ranges(current);
        let mut occurrence_counts = HashMap::<String, usize>::new();
        let mut dependencies = Vec::new();
        for handle in operands.into_iter().flatten() {
            let operand_ref = self.sibling_expr_ref(current, handle);
            let occurrence_range =
                self.operand_occurrence_range(&search_ranges, &operand_ref, &mut occurrence_counts);
            dependencies.extend(self.semantic_expr_dependencies_with_hint(
                operand_ref,
                edge_label.clone(),
                occurrence_range,
                env,
                target_stack,
                seen_exprs,
                definition_stack,
                depth + 1,
            ));
        }
        dependencies
    }

    fn projected_compose_dependencies(
        &self,
        current: &ExprRef,
        components: &[naga::Handle<Expression>],
        edge_label: Option<String>,
        projection: &AccessProjection,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        let search_ranges = self.operand_search_byte_ranges(current);
        let mut occurrence_counts = HashMap::<String, usize>::new();
        let mut dependencies = Vec::new();
        let mut output_offset = 0_u32;

        for handle in components {
            let operand_ref = self.sibling_expr_ref(current, *handle);
            let width = self.expression_component_count(&operand_ref).max(1) as u32;
            let selected_projection = projection
                .iter()
                .copied()
                .filter(|component| {
                    output_offset <= *component && *component < output_offset + width
                })
                .map(|component| component - output_offset)
                .collect::<Vec<_>>();
            output_offset += width;

            if selected_projection.is_empty() {
                continue;
            }

            let occurrence_range =
                self.operand_occurrence_range(&search_ranges, &operand_ref, &mut occurrence_counts);
            let child_projection = if width > 1 && selected_projection.len() < width as usize {
                Some(dedupe_projection(selected_projection))
            } else {
                None
            };
            if let Some(child_projection) = child_projection.as_ref() {
                dependencies.extend(self.semantic_expr_dependencies_projected(
                    operand_ref,
                    edge_label.clone(),
                    occurrence_range,
                    child_projection,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ));
            } else {
                dependencies.extend(self.semantic_expr_dependencies_with_hint(
                    operand_ref,
                    edge_label.clone(),
                    occurrence_range,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ));
            }
        }

        dependencies
    }

    fn projected_operand_dependencies<I>(
        &self,
        current: &ExprRef,
        operands: I,
        edge_label: Option<String>,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode>
    where
        I: IntoIterator<Item = (naga::Handle<Expression>, Option<AccessProjection>)>,
    {
        let search_ranges = self.operand_search_byte_ranges(current);
        let mut occurrence_counts = HashMap::<String, usize>::new();
        let mut dependencies = Vec::new();
        for (handle, projection) in operands {
            let operand_ref = self.sibling_expr_ref(current, handle);
            let occurrence_range =
                self.operand_occurrence_range(&search_ranges, &operand_ref, &mut occurrence_counts);
            if let Some(projection) = projection
                .as_ref()
                .filter(|projection| !projection.is_empty())
            {
                dependencies.extend(self.semantic_expr_dependencies_projected(
                    operand_ref,
                    edge_label.clone(),
                    occurrence_range,
                    projection,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ));
            } else {
                dependencies.extend(self.semantic_expr_dependencies_with_hint(
                    operand_ref,
                    edge_label.clone(),
                    occurrence_range,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ));
            }
        }
        dependencies
    }

    fn operand_projection(
        &self,
        current: &ExprRef,
        handle: naga::Handle<Expression>,
        projection: &AccessProjection,
    ) -> Option<AccessProjection> {
        let operand_ref = self.sibling_expr_ref(current, handle);
        let width = self.expression_component_count(&operand_ref);
        if width <= 1 {
            return None;
        }
        let projection = projection
            .iter()
            .copied()
            .filter(|component| (*component as usize) < width)
            .collect::<Vec<_>>();
        (!projection.is_empty()).then(|| dedupe_projection(projection))
    }

    fn operand_occurrence_range(
        &self,
        search_ranges: &[Range<usize>],
        operand_ref: &ExprRef,
        occurrence_counts: &mut HashMap<String, usize>,
    ) -> Option<PassDebugSourceRange> {
        if search_ranges.is_empty() {
            return None;
        }
        let label = self.reference_label_for_expr(operand_ref)?;
        let occurrence_index = occurrence_counts.entry(label.clone()).or_insert(0);
        let range = search_ranges.iter().find_map(|search_range| {
            find_identifier_occurrence_range(
                self.source,
                search_range.clone(),
                &label,
                *occurrence_index,
            )
        });
        *occurrence_index += 1;
        range
    }

    fn operand_search_byte_ranges(&self, current: &ExprRef) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        if let Some(range) = self.operand_search_byte_range(current) {
            ranges.push(range);
        }
        if let Some(line_range) = self
            .source_range_for_expr(current)
            .and_then(|range| source_line_byte_range(self.source, range.start_byte))
            && !ranges.iter().any(|range| range == &line_range)
        {
            ranges.push(line_range);
        }
        ranges
    }

    fn operand_search_byte_range(&self, current: &ExprRef) -> Option<Range<usize>> {
        let source_range = self.source_range_for_expr(current)?;
        let byte_range = source_range.start_byte..source_range.end_byte;
        if matches!(self.expression(current), Some(Expression::CallResult(_))) {
            find_enclosed_arguments_range(self.source, byte_range.clone()).or(Some(byte_range))
        } else {
            Some(byte_range)
        }
    }

    fn reference_label_for_expr(&self, expr_ref: &ExprRef) -> Option<String> {
        if let Some(target_id) = self.named_expression_target_id(expr_ref) {
            return self.target_name_for_id(&target_id);
        }
        self.access_path_target(expr_ref, &[])
            .map(|access_path| access_path.display_label)
    }

    fn access_path_target(
        &self,
        expr_ref: &ExprRef,
        target_stack: &[String],
    ) -> Option<AccessPathTarget> {
        let expr = self.expression(expr_ref)?;
        if let Some(target_id) = self.named_expression_target_id(expr_ref)
            && !target_stack.iter().any(|id| id == &target_id)
            && self.should_use_named_expression_access_path(expr_ref, expr, &target_id)
        {
            return Some(AccessPathTarget {
                display_label: self.target_name_for_id(&target_id)?,
                source_range: self
                    .source_range_for_expr(expr_ref)
                    .or_else(|| target_source_range(&self.targets, &target_id)),
                target_id,
                projection: None,
            });
        }

        match expr {
            Expression::FunctionArgument(index) => {
                let target_id = target_id_arg(expr_ref_scope(expr_ref), *index);
                let source_range = self
                    .source_range_for_expr(expr_ref)
                    .or_else(|| self.named_expression_value_source_range(expr_ref, &target_id));
                Some(AccessPathTarget {
                    display_label: self.target_name_for_id(&target_id)?,
                    target_id,
                    source_range,
                    projection: None,
                })
            }
            Expression::GlobalVariable(handle) => {
                let target_id = target_id_global(*handle);
                let source_range = self
                    .source_range_for_expr(expr_ref)
                    .or_else(|| self.named_expression_value_source_range(expr_ref, &target_id));
                Some(AccessPathTarget {
                    display_label: self.target_name_for_id(&target_id)?,
                    target_id,
                    source_range,
                    projection: None,
                })
            }
            Expression::LocalVariable(handle) => {
                let target_id = target_id_local(expr_ref_scope(expr_ref), *handle);
                let source_range = self
                    .source_range_for_expr(expr_ref)
                    .or_else(|| self.named_expression_value_source_range(expr_ref, &target_id));
                Some(AccessPathTarget {
                    display_label: self.target_name_for_id(&target_id)?,
                    target_id,
                    source_range,
                    projection: None,
                })
            }
            Expression::Load { pointer } => {
                let pointer_ref = self.sibling_expr_ref(expr_ref, *pointer);
                let mut path = self.access_path_target(&pointer_ref, target_stack)?;
                path.source_range = self
                    .access_path_source_range(expr_ref, &path.target_id)
                    .or(path.source_range);
                Some(path)
            }
            Expression::AccessIndex { base, index } => {
                let base_ref = self.sibling_expr_ref(expr_ref, *base);
                let mut path = self.access_path_target(&base_ref, target_stack)?;
                path.projection = self.access_index_projection(&base_ref, &path, *index);
                path.display_label
                    .push_str(&self.access_index_suffix(&base_ref, *index));
                path.source_range = self
                    .access_path_source_range(expr_ref, &path.target_id)
                    .or(path.source_range);
                Some(path)
            }
            Expression::Access { base, .. } => {
                let base_ref = self.sibling_expr_ref(expr_ref, *base);
                let mut path = self.access_path_target(&base_ref, target_stack)?;
                path.display_label.push_str("[]");
                path.source_range = self
                    .access_path_source_range(expr_ref, &path.target_id)
                    .or(path.source_range);
                Some(path)
            }
            Expression::Swizzle {
                size,
                vector,
                pattern,
            } => {
                let vector_ref = self.sibling_expr_ref(expr_ref, *vector);
                let mut path = self.access_path_target(&vector_ref, target_stack)?;
                path.projection = self.swizzle_projection(&vector_ref, &path, *size, pattern);
                path.display_label
                    .push_str(&format!(".{}", swizzle_pattern_label(*size, pattern)));
                path.source_range = self
                    .access_path_source_range(expr_ref, &path.target_id)
                    .or(path.source_range);
                Some(path)
            }
            _ => None,
        }
    }

    fn access_path_source_range(
        &self,
        expr_ref: &ExprRef,
        target_id: &str,
    ) -> Option<PassDebugSourceRange> {
        self.source_range_for_expr(expr_ref)
            .or_else(|| self.named_expression_value_source_range(expr_ref, target_id))
    }

    fn should_use_named_expression_access_path(
        &self,
        expr_ref: &ExprRef,
        expr: &Expression,
        named_target_id: &str,
    ) -> bool {
        let Some(underlying_target_id) = self.direct_access_target_id(expr_ref, expr) else {
            return true;
        };
        self.target_name_for_id(named_target_id) != self.target_name_for_id(&underlying_target_id)
    }

    fn direct_access_target_id(&self, expr_ref: &ExprRef, expr: &Expression) -> Option<String> {
        match expr {
            Expression::FunctionArgument(index) => {
                Some(target_id_arg(expr_ref_scope(expr_ref), *index))
            }
            Expression::GlobalVariable(handle) => Some(target_id_global(*handle)),
            Expression::LocalVariable(handle) => {
                Some(target_id_local(expr_ref_scope(expr_ref), *handle))
            }
            _ => None,
        }
    }

    fn named_expression_value_source_range(
        &self,
        expr_ref: &ExprRef,
        value_target_id: &str,
    ) -> Option<PassDebugSourceRange> {
        let named_target_id = self.named_expression_target_id(expr_ref)?;
        let named_range = target_source_range(&self.targets, &named_target_id)?;
        let value_name = self.target_name_for_id(value_target_id)?;
        let line_range = source_line_byte_range(self.source, named_range.start_byte)?;
        let rhs_start = named_range.end_byte.min(line_range.end);
        find_identifier_range(self.source, rhs_start..line_range.end, &value_name)
            .or_else(|| find_identifier_range(self.source, line_range, &value_name))
    }

    fn definition_value_occurrence_range(
        &self,
        definition_source_range: PassDebugSourceRange,
        value_ref: &ExprRef,
    ) -> Option<PassDebugSourceRange> {
        let label = self.reference_label_for_expr(value_ref)?;
        let line_range = source_line_byte_range(self.source, definition_source_range.start_byte)?;
        let rhs_start = definition_source_range.end_byte.min(line_range.end);
        find_identifier_range(self.source, rhs_start..line_range.end, &label)
            .or_else(|| find_identifier_range(self.source, line_range, &label))
    }

    fn access_path_index_dependencies(
        &self,
        expr_ref: &ExprRef,
        inherited_edge: Option<String>,
        env: Option<&DefinitionEnv>,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        definition_stack: &mut Vec<DefinitionId>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        let Some(expr) = self.expression(expr_ref) else {
            return Vec::new();
        };
        match expr {
            Expression::Load { pointer } => {
                let pointer_ref = self.sibling_expr_ref(expr_ref, *pointer);
                self.access_path_index_dependencies(
                    &pointer_ref,
                    inherited_edge,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
            }
            Expression::Access { base, index } => {
                let base_ref = self.sibling_expr_ref(expr_ref, *base);
                let index_ref = self.sibling_expr_ref(expr_ref, *index);
                let mut dependencies = self.access_path_index_dependencies(
                    &base_ref,
                    inherited_edge.clone(),
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                );
                dependencies.extend(self.semantic_expr_dependencies(
                    index_ref,
                    inherited_edge,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                ));
                dependencies
            }
            Expression::AccessIndex { base, .. } => {
                let base_ref = self.sibling_expr_ref(expr_ref, *base);
                self.access_path_index_dependencies(
                    &base_ref,
                    inherited_edge,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
            }
            Expression::Swizzle { vector, .. } => {
                let vector_ref = self.sibling_expr_ref(expr_ref, *vector);
                self.access_path_index_dependencies(
                    &vector_ref,
                    inherited_edge,
                    env,
                    target_stack,
                    seen_exprs,
                    definition_stack,
                    depth + 1,
                )
            }
            _ => Vec::new(),
        }
    }

    fn access_index_projection(
        &self,
        base_ref: &ExprRef,
        path: &AccessPathTarget,
        index: u32,
    ) -> Option<AccessProjection> {
        if let Some(projection) = path.projection.as_ref() {
            return projection
                .get(index as usize)
                .copied()
                .map(|component| vec![component]);
        }
        self.vector_width_for_expr(base_ref)
            .filter(|width| (index as usize) < *width)
            .map(|_| vec![index])
    }

    fn swizzle_projection(
        &self,
        vector_ref: &ExprRef,
        path: &AccessPathTarget,
        size: naga::VectorSize,
        pattern: &[SwizzleComponent; 4],
    ) -> Option<AccessProjection> {
        let swizzle_indices = pattern
            .iter()
            .take(vector_size_len(size))
            .filter_map(|component| swizzle_component_index(*component))
            .collect::<Vec<_>>();
        if swizzle_indices.is_empty() {
            return None;
        }

        if let Some(projection) = path.projection.as_ref() {
            let mapped = swizzle_indices
                .into_iter()
                .filter_map(|index| projection.get(index as usize).copied())
                .collect::<Vec<_>>();
            return Some(dedupe_projection(mapped));
        }

        self.vector_width_for_expr(vector_ref)
            .map(|_| dedupe_projection(swizzle_indices))
    }

    fn access_index_suffix(&self, base_ref: &ExprRef, index: u32) -> String {
        if let Some(member_name) = self.struct_member_name(base_ref, index) {
            format!(".{member_name}")
        } else if let Some(component) = swizzle_component_for_index(index) {
            format!(".{component}")
        } else {
            format!("[{index}]")
        }
    }

    fn vector_width_for_expr(&self, expr_ref: &ExprRef) -> Option<usize> {
        let ty = self.type_handle_for_expr(expr_ref)?;
        match &self.module.types[self.deref_type_handle(ty)].inner {
            TypeInner::Vector { size, .. } => Some(vector_size_len(*size)),
            _ => None,
        }
    }

    fn expression_component_count(&self, expr_ref: &ExprRef) -> usize {
        let Some(expr) = self.expression(expr_ref) else {
            return 1;
        };
        match expr {
            Expression::Compose { ty, .. } => self.type_component_count(*ty),
            Expression::Splat { size, .. } | Expression::Swizzle { size, .. } => {
                vector_size_len(*size)
            }
            Expression::Unary { expr, .. }
            | Expression::As { expr, .. }
            | Expression::Derivative { expr, .. } => {
                self.expression_component_count(&self.sibling_expr_ref(expr_ref, *expr))
            }
            Expression::Binary { left, right, .. } => self
                .expression_component_count(&self.sibling_expr_ref(expr_ref, *left))
                .max(self.expression_component_count(&self.sibling_expr_ref(expr_ref, *right))),
            Expression::Select { accept, reject, .. } => self
                .expression_component_count(&self.sibling_expr_ref(expr_ref, *accept))
                .max(self.expression_component_count(&self.sibling_expr_ref(expr_ref, *reject))),
            Expression::Math {
                arg,
                arg1,
                arg2,
                arg3,
                ..
            } => {
                let mut count =
                    self.expression_component_count(&self.sibling_expr_ref(expr_ref, *arg));
                for handle in [*arg1, *arg2, *arg3].into_iter().flatten() {
                    count = count.max(
                        self.expression_component_count(&self.sibling_expr_ref(expr_ref, handle)),
                    );
                }
                count
            }
            Expression::CallResult(_) => self.call_result_component_count(expr_ref).unwrap_or(1),
            _ => self
                .type_handle_for_expr(expr_ref)
                .map(|ty| self.type_component_count(ty))
                .unwrap_or(1),
        }
    }

    fn type_component_count(&self, ty: naga::Handle<Type>) -> usize {
        match &self.module.types[self.deref_type_handle(ty)].inner {
            TypeInner::Vector { size, .. } => vector_size_len(*size),
            _ => 1,
        }
    }

    fn call_result_component_count(&self, expr_ref: &ExprRef) -> Option<usize> {
        let ExprRef::Function { scope, handle } = expr_ref else {
            return None;
        };
        let call = self.calls_by_result.get(&(scope.clone(), handle.index()))?;
        let function = &self.module.functions[call.function];
        function
            .result
            .as_ref()
            .map(|result| self.type_component_count(result.ty))
    }

    fn struct_member_name(&self, base_ref: &ExprRef, index: u32) -> Option<String> {
        let ty = self.type_handle_for_expr(base_ref)?;
        let ty = self.deref_type_handle(ty);
        match &self.module.types[ty].inner {
            TypeInner::Struct { members, .. } => members
                .get(index as usize)
                .and_then(|member| member.name.clone())
                .or_else(|| Some(format!("field_{index}"))),
            _ => None,
        }
    }

    fn type_handle_for_expr(&self, expr_ref: &ExprRef) -> Option<naga::Handle<Type>> {
        let expr = self.expression(expr_ref)?;
        match expr {
            Expression::FunctionArgument(index) => {
                let function = self.functions.get(expr_ref_scope(expr_ref))?;
                function.arguments.get(*index as usize).map(|arg| arg.ty)
            }
            Expression::GlobalVariable(handle) => Some(self.module.global_variables[*handle].ty),
            Expression::LocalVariable(handle) => {
                let function = self.functions.get(expr_ref_scope(expr_ref))?;
                Some(function.local_variables[*handle].ty)
            }
            Expression::Load { pointer } => {
                let pointer_ref = self.sibling_expr_ref(expr_ref, *pointer);
                self.type_handle_for_expr(&pointer_ref)
                    .map(|ty| self.deref_type_handle(ty))
            }
            Expression::AccessIndex { base, index } => {
                let base_ref = self.sibling_expr_ref(expr_ref, *base);
                let base_ty = self.type_handle_for_expr(&base_ref)?;
                self.access_index_type_handle(base_ty, *index)
            }
            Expression::Access { base, .. } => {
                let base_ref = self.sibling_expr_ref(expr_ref, *base);
                let base_ty = self.type_handle_for_expr(&base_ref)?;
                self.indexed_type_handle(base_ty)
            }
            Expression::Compose { ty, .. } => Some(*ty),
            Expression::Splat { .. }
            | Expression::Swizzle { .. }
            | Expression::Unary { .. }
            | Expression::Binary { .. }
            | Expression::Select { .. }
            | Expression::Derivative { .. }
            | Expression::Relational { .. }
            | Expression::Math { .. }
            | Expression::As { .. }
            | Expression::CallResult(_)
            | Expression::ArrayLength(_)
            | Expression::RayQueryGetIntersection { .. } => None,
            _ => None,
        }
    }

    fn access_index_type_handle(
        &self,
        base_ty: naga::Handle<Type>,
        index: u32,
    ) -> Option<naga::Handle<Type>> {
        let base_ty = self.deref_type_handle(base_ty);
        match &self.module.types[base_ty].inner {
            TypeInner::Struct { members, .. } => {
                members.get(index as usize).map(|member| member.ty)
            }
            TypeInner::Array { base, .. } | TypeInner::BindingArray { base, .. } => Some(*base),
            _ => None,
        }
    }

    fn indexed_type_handle(&self, base_ty: naga::Handle<Type>) -> Option<naga::Handle<Type>> {
        let base_ty = self.deref_type_handle(base_ty);
        match &self.module.types[base_ty].inner {
            TypeInner::Array { base, .. } | TypeInner::BindingArray { base, .. } => Some(*base),
            _ => None,
        }
    }

    fn deref_type_handle(&self, ty: naga::Handle<Type>) -> naga::Handle<Type> {
        match self.module.types[ty].inner {
            TypeInner::Pointer { base, .. } => base,
            _ => ty,
        }
    }

    fn target_name_for_id(&self, target_id: &str) -> Option<String> {
        self.targets
            .iter()
            .find(|target| target.id == target_id)
            .map(|target| target.name.clone())
    }

    fn sibling_expr_ref(&self, current: &ExprRef, handle: naga::Handle<Expression>) -> ExprRef {
        match current {
            ExprRef::Global(_) => ExprRef::Global(handle),
            ExprRef::Function { scope, .. } => ExprRef::Function {
                scope: scope.clone(),
                handle,
            },
        }
    }

    fn store_lhs_source_range(
        &self,
        scope: &str,
        pointer_ref: &ExprRef,
        value_ref: &ExprRef,
        target_id: &str,
    ) -> Option<PassDebugSourceRange> {
        self.source_range_for_expr(pointer_ref).or_else(|| {
            let name = self.target_name_for_id(target_id)?;
            let value_range = self.source_range_for_expr(value_ref)?;
            let function_start = find_function_range(self.source, scope)
                .map(|range| range.start)
                .unwrap_or(0);
            let line_start = self.source[..value_range.start_byte]
                .rfind('\n')
                .map(|index| index + 1)
                .unwrap_or(function_start)
                .max(function_start);
            find_last_identifier_range(self.source, line_start..value_range.start_byte, &name)
                .or_else(|| {
                    let line_end = self.source[value_range.start_byte..]
                        .find('\n')
                        .map(|relative| value_range.start_byte + relative)
                        .unwrap_or(self.source.len());
                    find_identifier_range(self.source, line_start..line_end, &name)
                })
        })
    }

    fn next_store_lhs_source_range(
        &mut self,
        scope: &str,
        target_id: &str,
    ) -> Option<PassDebugSourceRange> {
        let name = self.target_name_for_id(target_id)?;
        let occurrence_index = self
            .store_lhs_counts_by_target
            .entry(target_id.to_string())
            .or_insert(0);
        let range = find_store_lhs_identifier_range(self.source, scope, &name, *occurrence_index);
        *occurrence_index += 1;
        range
    }

    fn resolve_pointer_target(&self, expr_ref: &ExprRef) -> Option<String> {
        let expr = self.expression(expr_ref)?;
        match expr {
            Expression::FunctionArgument(index) => {
                Some(target_id_arg(expr_ref_scope(expr_ref), *index))
            }
            Expression::GlobalVariable(handle) => Some(target_id_global(*handle)),
            Expression::LocalVariable(handle) => {
                Some(target_id_local(expr_ref_scope(expr_ref), *handle))
            }
            Expression::Access { base, .. } | Expression::AccessIndex { base, .. } => {
                self.resolve_pointer_target(&self.sibling_expr_ref(expr_ref, *base))
            }
            _ => None,
        }
    }

    fn named_expression_target_id(&self, expr_ref: &ExprRef) -> Option<String> {
        let ExprRef::Function { scope, handle } = expr_ref else {
            return None;
        };
        let function = self.functions.get(scope)?;
        if function.named_expressions.contains_key(handle) {
            Some(target_id_expr(scope, *handle))
        } else {
            None
        }
    }

    fn source_range_for_expr(&self, expr_ref: &ExprRef) -> Option<PassDebugSourceRange> {
        match expr_ref {
            ExprRef::Global(handle) => source_range_from_span(
                self.source,
                self.module.global_expressions.get_span(*handle),
            ),
            ExprRef::Function { scope, handle } => self.functions.get(scope).and_then(|function| {
                source_range_from_span(self.source, function.expressions.get_span(*handle))
            }),
        }
    }

    fn expression(&self, expr_ref: &ExprRef) -> Option<&Expression> {
        match expr_ref {
            ExprRef::Global(handle) => Some(&self.module.global_expressions[*handle]),
            ExprRef::Function { scope, handle } => self
                .functions
                .get(scope)
                .map(|function| &function.expressions[*handle]),
        }
    }
}

fn expr_ref_scope(expr_ref: &ExprRef) -> &str {
    match expr_ref {
        ExprRef::Global(_) => "module",
        ExprRef::Function { scope, .. } => scope.as_str(),
    }
}

fn operation_edge(inherited_edge: Option<String>, operation: impl Into<String>) -> Option<String> {
    inherited_edge.or_else(|| Some(operation.into()))
}

fn distinct_definition_source_range(
    definition_source_range: Option<PassDebugSourceRange>,
    source_range: Option<PassDebugSourceRange>,
) -> Option<PassDebugSourceRange> {
    definition_source_range.filter(|definition_range| Some(*definition_range) != source_range)
}

fn merge_definition_envs<I>(envs: I) -> DefinitionEnv
where
    I: IntoIterator<Item = DefinitionEnv>,
{
    let mut merged = DefinitionEnv::new();
    for env in envs {
        merge_definition_env_into(&mut merged, env);
    }
    merged
}

fn merge_definition_env_into(target: &mut DefinitionEnv, source: DefinitionEnv) {
    for (target_id, definitions) in source {
        let target_definitions = target.entry(target_id).or_default();
        for definition_id in definitions {
            if !target_definitions.contains(&definition_id) {
                target_definitions.push(definition_id);
            }
        }
    }
}

fn dependency_target_node_label(
    target: &PassDebugDependencyTarget,
    display_label: Option<&str>,
) -> String {
    let target_label = format!("{} ({})", target.label, target.kind);
    let Some(display_label) = display_label
        .map(str::trim)
        .filter(|label| !label.is_empty() && *label != target.name.as_str())
    else {
        return target_label;
    };
    format!("{display_label} -> {target_label}")
}

fn swizzle_pattern_label(size: naga::VectorSize, pattern: &[SwizzleComponent; 4]) -> String {
    pattern
        .iter()
        .take(vector_size_len(size))
        .filter_map(|component| swizzle_component_label(*component))
        .collect::<Vec<_>>()
        .join("")
}

fn vector_size_len(size: naga::VectorSize) -> usize {
    size as u8 as usize
}

fn swizzle_component_for_index(index: u32) -> Option<&'static str> {
    match index {
        0 => Some("x"),
        1 => Some("y"),
        2 => Some("z"),
        3 => Some("w"),
        _ => None,
    }
}

fn swizzle_component_index(component: SwizzleComponent) -> Option<u32> {
    match component {
        SwizzleComponent::X => Some(0),
        SwizzleComponent::Y => Some(1),
        SwizzleComponent::Z => Some(2),
        SwizzleComponent::W => Some(3),
    }
}

fn swizzle_component_label(component: SwizzleComponent) -> Option<&'static str> {
    match component {
        SwizzleComponent::X => Some("x"),
        SwizzleComponent::Y => Some("y"),
        SwizzleComponent::Z => Some("z"),
        SwizzleComponent::W => Some("w"),
    }
}

fn dedupe_projection(projection: AccessProjection) -> AccessProjection {
    projection
        .into_iter()
        .fold(Vec::new(), |mut deduped, component| {
            if !deduped.contains(&component) {
                deduped.push(component);
            }
            deduped
        })
}

fn combine_access_projection(
    access_projection: Option<&AccessProjection>,
    projection: &AccessProjection,
) -> AccessProjection {
    match access_projection {
        Some(access_projection) => dedupe_projection(
            projection
                .iter()
                .filter_map(|component| access_projection.get(*component as usize).copied())
                .collect(),
        ),
        None => dedupe_projection(projection.clone()),
    }
}

fn target_source_range(
    targets: &[PassDebugDependencyTarget],
    target_id: &str,
) -> Option<PassDebugSourceRange> {
    targets
        .iter()
        .find(|target| target.id == target_id)
        .and_then(|target| target.source_range)
}

fn source_range_from_span(source: &str, span: naga::Span) -> Option<PassDebugSourceRange> {
    span.to_range()
        .and_then(|range| source_range_from_byte_range(source, range))
}

fn source_range_from_byte_range(source: &str, range: Range<usize>) -> Option<PassDebugSourceRange> {
    if range.start >= range.end
        || range.end > source.len()
        || !source.is_char_boundary(range.start)
        || !source.is_char_boundary(range.end)
    {
        return None;
    }
    let start = u32::try_from(range.start).ok()?;
    let end = u32::try_from(range.end).ok()?;
    let location = naga::Span::new(start, end).location(source);
    Some(PassDebugSourceRange {
        start_byte: range.start,
        end_byte: range.end,
        line: location.line_number,
        column: location.line_position,
    })
}

fn source_line_byte_range(source: &str, byte_index: usize) -> Option<Range<usize>> {
    if byte_index > source.len() || !source.is_char_boundary(byte_index) {
        return None;
    }
    let start = source[..byte_index]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = source[byte_index..]
        .find('\n')
        .map(|relative| byte_index + relative)
        .unwrap_or(source.len());
    (start < end).then_some(start..end)
}

fn find_identifier_range(
    source: &str,
    range: Range<usize>,
    name: &str,
) -> Option<PassDebugSourceRange> {
    find_identifier_occurrence_range(source, range, name, 0)
}

fn find_last_identifier_range(
    source: &str,
    range: Range<usize>,
    name: &str,
) -> Option<PassDebugSourceRange> {
    if name.is_empty() || range.start >= range.end || range.end > source.len() {
        return None;
    }

    let haystack = &source[range.clone()];
    let mut offset = 0;
    let mut last = None;
    while let Some(relative) = haystack[offset..].find(name) {
        let start = range.start + offset + relative;
        let end = start + name.len();
        if is_identifier_start_boundary(source, start) && is_identifier_end_boundary(source, end) {
            last = source_range_from_byte_range(source, start..end);
        }
        offset += relative + name.len();
    }
    last
}

fn find_identifier_occurrence_range(
    source: &str,
    range: Range<usize>,
    name: &str,
    occurrence_index: usize,
) -> Option<PassDebugSourceRange> {
    if name.is_empty() || range.start >= range.end || range.end > source.len() {
        return None;
    }

    let haystack = &source[range.clone()];
    let mut offset = 0;
    let mut seen = 0usize;
    while let Some(relative) = haystack[offset..].find(name) {
        let start = range.start + offset + relative;
        let end = start + name.len();
        if is_identifier_start_boundary(source, start) && is_identifier_end_boundary(source, end) {
            if seen == occurrence_index {
                return source_range_from_byte_range(source, start..end);
            }
            seen += 1;
        }
        offset += relative + name.len();
    }
    None
}

fn find_global_identifier_range(source: &str, name: &str) -> Option<PassDebugSourceRange> {
    find_identifier_range(source, 0..source.len(), name)
}

fn find_argument_identifier_range(
    source: &str,
    scope: &str,
    name: &str,
) -> Option<PassDebugSourceRange> {
    let function_range = find_function_range(source, scope)?;
    let signature_end = source[function_range.clone()]
        .find('{')
        .map(|offset| function_range.start + offset)
        .unwrap_or(function_range.end);
    find_identifier_range(source, function_range.start..signature_end, name)
}

fn find_keyword_identifier_in_scope(
    source: &str,
    scope: &str,
    keyword: &str,
    name: &str,
) -> Option<PassDebugSourceRange> {
    let function_range = find_function_range(source, scope)?;
    find_keyword_identifier_range(source, function_range, keyword, name)
}

fn find_keyword_identifier_range(
    source: &str,
    range: Range<usize>,
    keyword: &str,
    name: &str,
) -> Option<PassDebugSourceRange> {
    if keyword.is_empty() || name.is_empty() || range.end > source.len() {
        return None;
    }

    let haystack = &source[range.clone()];
    let mut offset = 0;
    while let Some(relative) = haystack[offset..].find(keyword) {
        let keyword_start = range.start + offset + relative;
        let keyword_end = keyword_start + keyword.len();
        if is_identifier_start_boundary(source, keyword_start)
            && is_identifier_end_boundary(source, keyword_end)
        {
            let mut name_start = keyword_end;
            while name_start < range.end {
                let byte = source.as_bytes()[name_start];
                if byte.is_ascii_whitespace() {
                    name_start += 1;
                } else {
                    break;
                }
            }
            let name_end = name_start + name.len();
            if name_end <= range.end
                && &source[name_start..name_end] == name
                && is_identifier_start_boundary(source, name_start)
                && is_identifier_end_boundary(source, name_end)
            {
                return source_range_from_byte_range(source, name_start..name_end);
            }
        }
        offset += relative + keyword.len();
    }
    None
}

fn find_store_lhs_identifier_range(
    source: &str,
    scope: &str,
    name: &str,
    occurrence_index: usize,
) -> Option<PassDebugSourceRange> {
    let range = find_function_range(source, scope)?;
    if name.is_empty() || range.end > source.len() {
        return None;
    }

    let haystack = &source[range.clone()];
    let mut offset = 0;
    let mut seen = 0usize;
    while let Some(relative) = haystack[offset..].find(name) {
        let start = range.start + offset + relative;
        let end = start + name.len();
        if is_identifier_start_boundary(source, start)
            && is_identifier_end_boundary(source, end)
            && store_assignment_operator_start(source, end, range.end).is_some()
        {
            if seen == occurrence_index {
                return source_range_from_byte_range(source, start..end);
            }
            seen += 1;
        }
        offset += relative + name.len();
    }
    None
}

fn store_assignment_operator_start(source: &str, mut index: usize, end: usize) -> Option<usize> {
    index = skip_ascii_whitespace(source, index, end);
    while index < end {
        match source.as_bytes()[index] {
            b'.' => {
                index += 1;
                while index < end {
                    let byte = source.as_bytes()[index];
                    if byte.is_ascii_alphanumeric() || byte == b'_' {
                        index += 1;
                    } else {
                        break;
                    }
                }
                index = skip_ascii_whitespace(source, index, end);
            }
            b'[' => {
                index = skip_bracketed_source(source, index, end)?;
                index = skip_ascii_whitespace(source, index, end);
            }
            _ => break,
        }
    }

    let bytes = source.as_bytes();
    match bytes.get(index).copied()? {
        b'=' if bytes.get(index + 1) != Some(&b'=') => Some(index),
        b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^'
            if bytes.get(index + 1) == Some(&b'=') =>
        {
            Some(index)
        }
        b'<' if bytes.get(index + 1) == Some(&b'<') && bytes.get(index + 2) == Some(&b'=') => {
            Some(index)
        }
        b'>' if bytes.get(index + 1) == Some(&b'>') && bytes.get(index + 2) == Some(&b'=') => {
            Some(index)
        }
        _ => None,
    }
}

fn skip_ascii_whitespace(source: &str, mut index: usize, end: usize) -> usize {
    while index < end && source.as_bytes()[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn skip_bracketed_source(source: &str, open: usize, end: usize) -> Option<usize> {
    if source.as_bytes().get(open) != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    for index in open..end {
        match source.as_bytes()[index] {
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_function_range(source: &str, scope: &str) -> Option<Range<usize>> {
    let mut offset = 0;
    while let Some(relative) = source[offset..].find("fn") {
        let fn_start = offset + relative;
        let fn_end = fn_start + 2;
        if !is_identifier_start_boundary(source, fn_start)
            || !is_identifier_end_boundary(source, fn_end)
        {
            offset = fn_end;
            continue;
        }

        let mut name_start = fn_end;
        while name_start < source.len() && source.as_bytes()[name_start].is_ascii_whitespace() {
            name_start += 1;
        }
        let name_end = name_start + scope.len();
        if name_end <= source.len()
            && &source[name_start..name_end] == scope
            && is_identifier_start_boundary(source, name_start)
            && is_identifier_end_boundary(source, name_end)
        {
            let body_start = source[name_end..]
                .find('{')
                .map(|relative| name_end + relative);
            let function_end = body_start
                .and_then(|start| find_matching_brace(source, start))
                .map(|end| end + 1)
                .unwrap_or(source.len());
            return Some(fn_start..function_end);
        }

        offset = fn_end;
    }
    None
}

fn find_matching_brace(source: &str, open_brace: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, byte) in source.as_bytes().iter().enumerate().skip(open_brace) {
        match *byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_enclosed_arguments_range(source: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start >= range.end || range.end > source.len() {
        return None;
    }
    let open = source.as_bytes()[range.clone()]
        .iter()
        .position(|byte| *byte == b'(')
        .map(|relative| range.start + relative)?;
    let mut depth = 0usize;
    for index in open..range.end {
        match source.as_bytes()[index] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(open + 1..index);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_identifier_start_boundary(source: &str, byte_index: usize) -> bool {
    if byte_index > source.len() {
        return false;
    }
    !byte_index
        .checked_sub(1)
        .and_then(|index| source.as_bytes().get(index))
        .copied()
        .map(is_wgsl_identifier_byte)
        .unwrap_or(false)
}

fn is_identifier_end_boundary(source: &str, byte_index: usize) -> bool {
    if byte_index > source.len() {
        return false;
    }
    !source
        .as_bytes()
        .get(byte_index)
        .copied()
        .map(is_wgsl_identifier_byte)
        .unwrap_or(false)
}

fn is_wgsl_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

fn build_dependency_debug(module: &Module, source: &str) -> DependencyDebugBuild {
    DependencyAnalyzer::new(module, source).into_debug()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{PassDebugDependencyNode, PassDebugSource};

    fn target_id_by_name(doc: &PassDebugSource, name: &str) -> String {
        doc.dependency_targets
            .iter()
            .find(|target| target.name == name)
            .unwrap_or_else(|| panic!("missing dependency target named {name}"))
            .id
            .clone()
    }

    fn assert_target_range_selects_name(doc: &PassDebugSource, name: &str) {
        let target = doc
            .dependency_targets
            .iter()
            .find(|target| target.name == name)
            .unwrap_or_else(|| panic!("missing dependency target named {name}"));
        let range = target
            .source_range
            .unwrap_or_else(|| panic!("missing source range for target named {name}"));
        assert_eq!(&doc.module_source[range.start_byte..range.end_byte], name);
        assert!(range.line > 0);
        assert!(range.column > 0);
    }

    fn flatten_dependency_labels(node: &PassDebugDependencyNode, out: &mut Vec<String>) {
        out.push(node.label.clone());
        for child in &node.children {
            flatten_dependency_labels(child, out);
        }
    }

    fn dependency_labels_for(doc: &PassDebugSource, name: &str) -> Vec<String> {
        let id = target_id_by_name(doc, name);
        let tree = doc
            .dependency_trees
            .get(&id)
            .unwrap_or_else(|| panic!("missing dependency tree for target {name}"));
        let mut labels = Vec::new();
        flatten_dependency_labels(tree, &mut labels);
        labels
    }

    fn assert_labels_contain(labels: &[String], needle: &str) {
        assert!(
            labels.iter().any(|label| label.contains(needle)),
            "expected dependency labels to contain `{needle}`\nlabels:\n{}",
            labels.join("\n")
        );
    }

    fn assert_labels_do_not_contain(labels: &[String], needle: &str) {
        assert!(
            labels.iter().all(|label| !label.contains(needle)),
            "expected dependency labels not to contain `{needle}`\nlabels:\n{}",
            labels.join("\n")
        );
    }

    fn target_name_for_id<'a>(doc: &'a PassDebugSource, target_id: &str) -> &'a str {
        doc.dependency_targets
            .iter()
            .find(|target| target.id == target_id)
            .map(|target| target.name.as_str())
            .unwrap_or_else(|| panic!("missing dependency target id {target_id}"))
    }

    fn child_target<'a>(
        doc: &PassDebugSource,
        node: &'a PassDebugDependencyNode,
        name: &str,
        edge_label: Option<&str>,
    ) -> &'a PassDebugDependencyNode {
        node.children
            .iter()
            .find(|child| {
                child
                    .target_id
                    .as_deref()
                    .map(|target_id| target_name_for_id(doc, target_id) == name)
                    .unwrap_or(false)
                    && child.edge_label.as_deref() == edge_label
            })
            .unwrap_or_else(|| {
                let children = node
                    .children
                    .iter()
                    .map(|child| {
                        let target = child
                            .target_id
                            .as_deref()
                            .map(|target_id| target_name_for_id(doc, target_id))
                            .unwrap_or("<non-target>");
                        format!("{target} edge={:?} label={}", child.edge_label, child.label)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                panic!("missing child target {name} edge={edge_label:?}\nchildren:\n{children}")
            })
    }

    fn collect_target_nodes_by_name<'a>(
        doc: &PassDebugSource,
        node: &'a PassDebugDependencyNode,
        name: &str,
        out: &mut Vec<&'a PassDebugDependencyNode>,
    ) {
        if node
            .target_id
            .as_deref()
            .map(|target_id| target_name_for_id(doc, target_id) == name)
            .unwrap_or(false)
        {
            out.push(node);
        }
        for child in &node.children {
            collect_target_nodes_by_name(doc, child, name, out);
        }
    }

    fn collect_target_nodes_on_line<'a>(
        node: &'a PassDebugDependencyNode,
        target_id: &str,
        line: u32,
        out: &mut Vec<&'a PassDebugDependencyNode>,
    ) {
        if node.target_id.as_deref() == Some(target_id)
            && node
                .source_range
                .is_some_and(|source_range| source_range.line == line)
        {
            out.push(node);
        }
        for child in &node.children {
            collect_target_nodes_on_line(child, target_id, line, out);
        }
    }

    fn dependency_graph_reaches_target(
        doc: &PassDebugSource,
        start_target_id: &str,
        wanted_target_id: &str,
    ) -> bool {
        fn visit_node(
            doc: &PassDebugSource,
            node: &PassDebugDependencyNode,
            wanted_target_id: &str,
            seen_targets: &mut HashSet<String>,
        ) -> bool {
            if let Some(target_id) = node.target_id.as_deref() {
                if target_id == wanted_target_id {
                    return true;
                }
                if seen_targets.insert(target_id.to_string())
                    && let Some(tree) = doc.dependency_trees.get(target_id)
                    && visit_node(doc, tree, wanted_target_id, seen_targets)
                {
                    return true;
                }
            }
            node.children
                .iter()
                .any(|child| visit_node(doc, child, wanted_target_id, seen_targets))
        }

        doc.dependency_trees
            .get(start_target_id)
            .map(|tree| visit_node(doc, tree, wanted_target_id, &mut HashSet::new()))
            .unwrap_or(false)
    }

    fn assert_node_range_selects(
        doc: &PassDebugSource,
        node: &PassDebugDependencyNode,
        expected_start_byte: usize,
        expected_text: &str,
    ) {
        let range = node
            .source_range
            .unwrap_or_else(|| panic!("missing node source range for {}", node.label));
        assert_eq!(range.start_byte, expected_start_byte);
        assert_eq!(
            &doc.module_source[range.start_byte..range.end_byte],
            expected_text
        );
    }

    #[test]
    fn valid_wgsl_builds_non_empty_ast() {
        let source = r#"
@vertex
fn vs_main(@location(0) position: vec3f) -> @builtin(position) vec4f {
    return vec4f(position, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4f {
    return vec4f(1.0);
}
"#;

        let doc = PassDebugSource::from_wgsl("test.pass", source);

        assert!(doc.parse_error.is_none());
        assert!(!doc.ast_tree.is_empty());
        assert!(
            doc.ast_tree
                .iter()
                .any(|n| n.label.starts_with("Entry Points"))
        );
    }

    #[test]
    fn invalid_wgsl_preserves_source_and_reports_error() {
        let source = "fn nope() -> { return vec4f(1.0); }";

        let doc = PassDebugSource::from_wgsl("bad.pass", source);

        assert_eq!(doc.module_source, source);
        assert!(doc.parse_error.is_some());
        assert_eq!(doc.ast_tree[0].label, "Parse Error");
    }

    #[test]
    fn dependency_targets_include_named_lets_locals_args_and_globals() {
        let source = r#"
@group(0) @binding(0) var<uniform> threshold: f32;

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    var x: f32 = 0.0;
    let y = uv.x + threshold;
    x = y;
    return vec4f(x, x, x, 1.0);
}
"#;

        let doc = PassDebugSource::from_wgsl("deps.pass", source);

        assert!(doc.parse_error.is_none());
        assert!(
            doc.dependency_targets
                .iter()
                .any(|target| target.name == "threshold")
        );
        assert!(
            doc.dependency_targets
                .iter()
                .any(|target| target.name == "uv")
        );
        assert!(
            doc.dependency_targets
                .iter()
                .any(|target| target.name == "x")
        );
        assert!(
            doc.dependency_targets
                .iter()
                .any(|target| target.name == "y")
        );

        let x_labels = dependency_labels_for(&doc, "x");
        assert_labels_contain(&x_labels, "fs_main let y");
        assert_labels_do_not_contain(&x_labels, "[cycle]");

        let y_labels = dependency_labels_for(&doc, "y");
        assert_labels_contain(&y_labels, "argument uv");
        assert_labels_contain(&y_labels, "global threshold");
    }

    #[test]
    fn dependency_targets_include_source_ranges_for_names() {
        let source = r#"
@group(0) @binding(0) var<uniform> threshold: f32;

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    var x: f32 = 0.0;
    let y = uv.x + threshold;
    return vec4f(y, x, x, 1.0);
}
"#;

        let doc = PassDebugSource::from_wgsl("ranges.pass", source);

        assert!(doc.parse_error.is_none());
        for name in ["threshold", "uv", "x", "y"] {
            assert_target_range_selects_name(&doc, name);
        }
    }

    #[test]
    fn control_flow_conditions_contribute_to_stores() {
        let source = r#"
fn choose(i: i32) -> f32 {
    var x: f32 = 0.0;
    if i > 0 {
        x = 1.0;
    }
    switch i {
        case 2: {
            x = 2.0;
        }
        default: {}
    }
    return x;
}
"#;

        let doc = PassDebugSource::from_wgsl("control.pass", source);
        assert!(doc.parse_error.is_none());

        let labels = dependency_labels_for(&doc, "x");
        assert_labels_contain(&labels, "[condition] if");
        assert_labels_contain(&labels, "[condition] switch selector");
        assert_labels_contain(&labels, "[condition] case");
    }

    #[test]
    fn function_call_dependencies_are_variable_map_edges() {
        let source = r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn math_multiply(a: f32, e: f32) -> f32 {
    return a * e;
}

fn debug_main() -> f32 {
    let b = 1.0;
    let c = 2.0;
    let e = 3.0;
    let a = foo(b, c);
    let d = math_multiply(a, e);
    return d;
}
"#;

        let doc = PassDebugSource::from_wgsl("call.pass", source);
        assert!(doc.parse_error.is_none());

        let d_id = target_id_by_name(&doc, "d");
        let d = doc
            .dependency_trees
            .get(&d_id)
            .expect("missing dependency tree for d");
        assert_eq!(d.edge_label.as_deref(), None);

        let a = child_target(&doc, d, "a", Some("math_multiply"));
        child_target(&doc, d, "e", Some("math_multiply"));
        assert!(a.reference);
        let a_tree = doc
            .dependency_trees
            .get(a.target_id.as_ref().expect("missing a target id"))
            .expect("missing dependency tree for a");
        child_target(&doc, a_tree, "b", Some("foo"));
        child_target(&doc, a_tree, "c", Some("foo"));
    }

    #[test]
    fn named_expression_dependency_nodes_point_to_reference_occurrences() {
        let source = r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(a: f32, c: f32) -> f32 {
    return a + c;
}

fn debug_main() -> f32 {
    let b = 1.0;
    let c = 2.0;
    let a = foo(b, c);
    let d = bar(a, c);
    return d;
}
"#;

        let doc = PassDebugSource::from_wgsl("reference-occurrence.pass", source);
        assert!(doc.parse_error.is_none());

        let d_id = target_id_by_name(&doc, "d");
        let d = doc
            .dependency_trees
            .get(&d_id)
            .expect("missing dependency tree for d");
        let a = child_target(&doc, d, "a", Some("bar"));
        assert!(a.reference);
        let a_tree = doc
            .dependency_trees
            .get(a.target_id.as_ref().expect("missing a target id"))
            .expect("missing dependency tree for a");
        let b = child_target(&doc, a_tree, "b", Some("foo"));

        let d_arg_start = doc.module_source.find("bar(a, c)").unwrap() + "bar(".len();
        assert_node_range_selects(&doc, a, d_arg_start, "a");

        let a_arg_start = doc.module_source.find("foo(b, c)").unwrap() + "foo(".len();
        assert_node_range_selects(&doc, b, a_arg_start, "b");
    }

    #[test]
    fn duplicate_named_expression_operands_keep_distinct_occurrence_ranges() {
        let source = r#"
fn bar(left: f32, right: f32) -> f32 {
    return left + right;
}

fn debug_main() -> f32 {
    let a = 1.0;
    let d = bar(a, a);
    return d;
}
"#;

        let doc = PassDebugSource::from_wgsl("duplicate-reference.pass", source);
        assert!(doc.parse_error.is_none());

        let d_id = target_id_by_name(&doc, "d");
        let d = doc
            .dependency_trees
            .get(&d_id)
            .expect("missing dependency tree for d");
        let a_children = d
            .children
            .iter()
            .filter(|child| {
                child.edge_label.as_deref() == Some("bar")
                    && child
                        .target_id
                        .as_deref()
                        .map(|target_id| target_name_for_id(&doc, target_id) == "a")
                        .unwrap_or(false)
            })
            .collect::<Vec<_>>();
        assert_eq!(a_children.len(), 2);

        let call_start = doc.module_source.find("bar(a, a)").unwrap();
        assert_node_range_selects(&doc, a_children[0], call_start + "bar(".len(), "a");
        assert_node_range_selects(&doc, a_children[1], call_start + "bar(a, ".len(), "a");
    }

    #[test]
    fn reassigned_reference_nodes_keep_occurrence_and_reaching_definition_ranges() {
        let source = r#"
fn fun(v: f32) -> f32 {
    return v;
}

fn foo(v: f32) -> f32 {
    return v;
}

fn bar(v: f32) -> f32 {
    return v;
}

@fragment
fn fs_main() -> @location(0) f32 {
    var a: f32 = 1.0;
    a = fun(a);
    let b = foo(a);
    let c = bar(a);
    return b + c;
}
"#;

        let doc = PassDebugSource::from_wgsl("reassigned-reference.pass", source);
        assert!(doc.parse_error.is_none());

        let b_id = target_id_by_name(&doc, "b");
        let b = doc
            .dependency_trees
            .get(&b_id)
            .expect("missing dependency tree for b");
        let a_from_b = child_target(&doc, b, "a", Some("foo"));
        let foo_arg_start = doc.module_source.find("foo(a)").unwrap() + "foo(".len();
        let store_start = doc.module_source.find("a = fun(a);").unwrap();
        assert_node_range_selects(&doc, a_from_b, foo_arg_start, "a");
        let definition_range = a_from_b
            .definition_source_range
            .expect("expected foo(a) to expose reaching definition");
        assert_eq!(definition_range.start_byte, store_start);
        assert_eq!(
            &doc.module_source[definition_range.start_byte..definition_range.end_byte],
            "a"
        );

        let a_id = target_id_by_name(&doc, "a");
        let a_tree = doc
            .dependency_trees
            .get(&a_id)
            .expect("missing dependency tree for a");
        let fun_arg_start = doc.module_source.find("a = fun(a);").unwrap() + "a = fun(".len();
        let declaration_start = doc.module_source.find("var a").unwrap() + "var ".len();
        let mut a_nodes = Vec::new();
        collect_target_nodes_by_name(&doc, a_tree, "a", &mut a_nodes);
        let nested_a = a_nodes
            .into_iter()
            .find(|node| {
                node.source_range
                    .is_some_and(|range| range.start_byte == fun_arg_start)
            })
            .expect("missing nested fun(a) target node");
        assert_node_range_selects(&doc, nested_a, fun_arg_start, "a");
        let nested_definition_range = nested_a
            .definition_source_range
            .expect("expected fun(a) to expose previous definition");
        assert_eq!(nested_definition_range.start_byte, declaration_start);
        assert_eq!(
            &doc.module_source
                [nested_definition_range.start_byte..nested_definition_range.end_byte],
            "a"
        );
    }

    #[test]
    fn local_reference_node_points_to_occurrence_and_keeps_definition_children() {
        let source = r#"
@fragment
fn fs_main() -> @location(0) f32 {
    let edge = 30.0;
    let edge_sdf = edge + 1.0;
    let aa_depth = edge * 2.0;
    var final_alpha = smoothstep(0.0, aa_depth, -edge_sdf);
    let out = 0.5 * final_alpha;
    return out;
}
"#;

        let doc = PassDebugSource::from_wgsl("final-alpha.pass", source);
        assert!(doc.parse_error.is_none());

        let out_id = target_id_by_name(&doc, "out");
        let out = doc
            .dependency_trees
            .get(&out_id)
            .expect("missing dependency tree for out");
        let final_alpha = child_target(&doc, out, "final_alpha", Some("Multiply"));

        let occurrence_start =
            doc.module_source.find("0.5 * final_alpha").unwrap() + "0.5 * ".len();
        assert_node_range_selects(&doc, final_alpha, occurrence_start, "final_alpha");
        assert!(final_alpha.reference);

        let mut labels = Vec::new();
        let final_alpha_tree = doc
            .dependency_trees
            .get(
                final_alpha
                    .target_id
                    .as_ref()
                    .expect("missing final_alpha target id"),
            )
            .expect("missing dependency tree for final_alpha");
        flatten_dependency_labels(final_alpha_tree, &mut labels);
        assert_labels_contain(&labels, "aa_depth");
        assert_labels_contain(&labels, "edge_sdf");
    }

    #[test]
    fn compound_vector_self_assignment_keeps_rhs_contributors() {
        let source = r#"
@fragment
fn fs_main() -> @location(0) vec4f {
    var x = vec4f(0.0);
    let lighting1 = 0.25;
    let lighting2 = 0.5;
    x = vec4f(x.rgb, x.a);
    x += lighting1 + lighting2;
    x = vec4f(x.rgb * x.a, x.a);
    return x;
}
"#;

        let doc = PassDebugSource::from_wgsl("compound.pass", source);
        assert!(doc.parse_error.is_none());

        let x_id = target_id_by_name(&doc, "x");
        let x = doc
            .dependency_trees
            .get(&x_id)
            .expect("missing dependency tree for x");
        let mut labels = Vec::new();
        flatten_dependency_labels(x, &mut labels);
        assert_labels_contain(&labels, "lighting1");
        assert_labels_contain(&labels, "lighting2");

        let mut x_nodes = Vec::new();
        collect_target_nodes_by_name(&doc, x, "x", &mut x_nodes);
        let alpha_occurrences = x_nodes
            .iter()
            .filter_map(|node| node.source_range)
            .filter(|range| &doc.module_source[range.start_byte..range.end_byte] == "x.a")
            .collect::<Vec<_>>();
        assert!(
            alpha_occurrences.len() >= 2,
            "expected distinct x.a occurrences in dependency tree"
        );
        assert_ne!(alpha_occurrences[0], alpha_occurrences[1]);
    }

    #[test]
    fn projected_self_assignment_through_call_reaches_previous_definition() {
        let source = r#"
fn adjust(v: vec4f) -> vec4f {
    return v;
}

@fragment
fn fs_main() -> @location(0) vec4f {
    let base = vec4f(0.25);
    var x = base;
    x = adjust(x);
    x = vec4f(x.rgb, x.a);
    return x;
}
"#;

        let doc = PassDebugSource::from_wgsl("projected-self-call.pass", source);
        assert!(doc.parse_error.is_none());

        let x_labels = dependency_labels_for(&doc, "x");
        assert_labels_contain(&x_labels, "base");
        assert_labels_do_not_contain(&x_labels, "[source] no contributors");

        let x_id = target_id_by_name(&doc, "x");
        let x = doc
            .dependency_trees
            .get(&x_id)
            .expect("missing dependency tree for x");
        let mut x_occurrences = Vec::new();
        collect_target_nodes_on_line(x, &x_id, 11, &mut x_occurrences);
        assert!(
            x_occurrences.iter().any(|node| node
                .definition_source_range
                .is_some_and(|source_range| source_range.line == 10)),
            "line 11 x references should expose the previous definition on line 10"
        );
    }

    #[test]
    fn glass_mat_dependency_tree_keeps_lighting_and_distinct_alpha_occurrences() {
        let source = std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("cases")
                .join("glass")
                .join("wgsl")
                .join("RenderPass_4.module.wgsl"),
        )
        .expect("failed to read glass RenderPass_4 WGSL");
        let doc = PassDebugSource::from_wgsl("glass.pass", source);
        assert!(doc.parse_error.is_none());

        let glass_id = target_id_by_name(&doc, "glass_mat");
        let glass_mat = doc
            .dependency_trees
            .get(&glass_id)
            .expect("missing dependency tree for glass_mat");
        let mut glass_labels = Vec::new();
        flatten_dependency_labels(glass_mat, &mut glass_labels);
        assert_labels_do_not_contain(&glass_labels, "[depth limit]");

        let mut lighting1_nodes = Vec::new();
        let mut lighting2_nodes = Vec::new();
        collect_target_nodes_by_name(&doc, glass_mat, "lighting1", &mut lighting1_nodes);
        collect_target_nodes_by_name(&doc, glass_mat, "lighting2", &mut lighting2_nodes);
        assert!(
            !lighting1_nodes.is_empty(),
            "glass_mat dependency tree should include lighting1"
        );
        assert!(
            !lighting2_nodes.is_empty(),
            "glass_mat dependency tree should include lighting2"
        );
        assert!(
            lighting1_nodes
                .iter()
                .all(|node| !node.label.contains("[depth limit]")),
            "lighting1 target references should not be labeled as depth-limited"
        );
        assert!(
            lighting2_nodes
                .iter()
                .all(|node| !node.label.contains("[depth limit]")),
            "lighting2 target references should not be labeled as depth-limited"
        );
        let lighting1_id = target_id_by_name(&doc, "lighting1");
        let lighting1 = doc
            .dependency_trees
            .get(&lighting1_id)
            .expect("missing dependency tree for lighting1");
        let mut lighting1_labels = Vec::new();
        flatten_dependency_labels(lighting1, &mut lighting1_labels);
        assert_labels_do_not_contain(&lighting1_labels, "[depth limit]");
        assert!(
            !lighting1.children.is_empty(),
            "lighting1 canonical dependency tree should remain expandable"
        );

        let lighting2_id = target_id_by_name(&doc, "lighting2");
        let lighting2 = doc
            .dependency_trees
            .get(&lighting2_id)
            .expect("missing dependency tree for lighting2");
        let mut lighting2_labels = Vec::new();
        flatten_dependency_labels(lighting2, &mut lighting2_labels);
        assert_labels_do_not_contain(&lighting2_labels, "[depth limit]");
        assert!(
            !lighting2.children.is_empty(),
            "lighting2 canonical dependency tree should remain expandable"
        );

        let root_id = doc
            .dependency_root_target_id
            .as_ref()
            .expect("missing root dependency target");
        let root = doc
            .dependency_trees
            .get(root_id)
            .expect("missing root dependency tree");
        let mut root_labels = Vec::new();
        flatten_dependency_labels(root, &mut root_labels);
        assert_labels_do_not_contain(&root_labels, "[depth limit]");
        assert!(
            dependency_graph_reaches_target(&doc, root_id, &lighting1_id),
            "root dependency graph should reach lighting1 through target references"
        );
        assert!(
            dependency_graph_reaches_target(&doc, root_id, &lighting2_id),
            "root dependency graph should reach lighting2 through target references"
        );
        let frag_out = child_target(&doc, root, "_frag_out", Some("Compose"));
        assert_node_range_selects(
            &doc,
            frag_out,
            doc.module_source.find("_frag_out.rgb").unwrap(),
            "_frag_out.rgb",
        );
        let frag_out_definition_range = frag_out
            .definition_source_range
            .expect("expected _frag_out source jump range");
        assert_eq!(frag_out_definition_range.line, 764);
        assert_eq!(
            &doc.module_source
                [frag_out_definition_range.start_byte..frag_out_definition_range.end_byte],
            "_frag_out"
        );

        let frag_out_id = target_id_by_name(&doc, "_frag_out");
        let frag_out_tree = doc
            .dependency_trees
            .get(&frag_out_id)
            .expect("missing dependency tree for _frag_out");
        let material_out = child_target(&doc, frag_out_tree, "glass_material_material_out", None);
        assert_node_range_selects(
            &doc,
            material_out,
            doc.module_source
                .find("let _frag_out = glass_material_material_out")
                .unwrap()
                + "let _frag_out = ".len(),
            "glass_material_material_out",
        );
        let material_out_definition_range = material_out
            .definition_source_range
            .expect("expected glass_material_material_out source jump range");
        assert_eq!(material_out_definition_range.line, 760);
        assert_eq!(
            &doc.module_source
                [material_out_definition_range.start_byte..material_out_definition_range.end_byte],
            "glass_material_material_out"
        );

        let material_out_id = target_id_by_name(&doc, "glass_material_material_out");
        let material_out_tree = doc
            .dependency_trees
            .get(&material_out_id)
            .expect("missing dependency tree for glass_material_material_out");
        let material_out_definition =
            child_target(&doc, material_out_tree, "glass_material_material_out", None);
        assert_eq!(
            material_out_definition
                .source_range
                .expect("expected material_out definition occurrence")
                .line,
            760
        );
        let material_value = child_target(&doc, material_out_definition, "glass_mat", None);
        assert_node_range_selects(
            &doc,
            material_value,
            doc.module_source
                .find("glass_material_material_out = glass_mat")
                .unwrap()
                + "glass_material_material_out = ".len(),
            "glass_mat",
        );
        let material_value_definition_range = material_value
            .definition_source_range
            .expect("expected glass_mat source jump range from material_out");
        assert_eq!(material_value_definition_range.line, 759);

        let mut glass_nodes = Vec::new();
        collect_target_nodes_by_name(&doc, glass_mat, "glass_mat", &mut glass_nodes);
        for line in [741, 740, 739] {
            let mut line_nodes = Vec::new();
            collect_target_nodes_on_line(glass_mat, &glass_id, line, &mut line_nodes);
            assert!(
                !line_nodes.is_empty(),
                "expected glass_mat dependency tree to include line {line}"
            );
        }
        let mut line_707_nodes = Vec::new();
        collect_target_nodes_on_line(glass_mat, &glass_id, 707, &mut line_707_nodes);
        assert!(
            line_707_nodes.iter().any(|node| node
                .definition_source_range
                .is_some_and(|source_range| source_range.line == 706)),
            "line 707 glass_mat references should expose the initial definition on line 706"
        );
        for (name, definition_line) in [("refraction", 697), ("reflection", 703)] {
            let mut nodes = Vec::new();
            collect_target_nodes_by_name(&doc, glass_mat, name, &mut nodes);
            assert!(
                nodes
                    .iter()
                    .any(|node| node.source_range.is_some_and(|source_range| {
                        source_range.line == 706
                            && &doc.module_source[source_range.start_byte..source_range.end_byte]
                                == name
                    })),
                "{name} should keep its line 706 mix() occurrence"
            );
            assert!(
                nodes.iter().any(
                    |node| node.definition_source_range.is_some_and(|source_range| {
                        source_range.line == definition_line
                            && &doc.module_source[source_range.start_byte..source_range.end_byte]
                                == name
                    })
                ),
                "{name} should expose its reaching definition line {definition_line}"
            );
        }

        let alpha_759_ranges = glass_nodes
            .iter()
            .filter_map(|node| node.source_range)
            .filter(|range| {
                range.line == 759
                    && &doc.module_source[range.start_byte..range.end_byte] == "glass_mat.a"
            })
            .collect::<Vec<_>>();
        assert_eq!(
            alpha_759_ranges.len(),
            2,
            "expected two distinct alpha references on line 759"
        );
        assert_ne!(alpha_759_ranges[0], alpha_759_ranges[1]);
    }

    #[test]
    fn function_call_argument_edges_keep_argument_dependency_trees_per_call_site() {
        let source = r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(b: f32, c: f32) -> f32 {
    return b - c;
}

fn debug_main() -> f32 {
    let source_b = 1.0;
    let source_c = 2.0;
    let b = source_b + 10.0;
    let c = source_c + 20.0;
    let a = foo(b, c);
    let d = bar(b, c);
    return a + d;
}
"#;

        let doc = PassDebugSource::from_wgsl("call-args.pass", source);
        assert!(doc.parse_error.is_none());

        let a_id = target_id_by_name(&doc, "a");
        let a = doc
            .dependency_trees
            .get(&a_id)
            .expect("missing dependency tree for a");
        let a_b = child_target(&doc, a, "b", Some("foo"));
        let a_c = child_target(&doc, a, "c", Some("foo"));
        assert!(a_b.reference);
        assert!(a_c.reference);

        let d_id = target_id_by_name(&doc, "d");
        let d = doc
            .dependency_trees
            .get(&d_id)
            .expect("missing dependency tree for d");
        let d_b = child_target(&doc, d, "b", Some("bar"));
        let d_c = child_target(&doc, d, "c", Some("bar"));
        assert!(d_b.reference);
        assert!(d_c.reference);

        let b_id = a_b
            .target_id
            .as_ref()
            .expect("missing b reference target id");
        let b = doc
            .dependency_trees
            .get(b_id)
            .expect("missing dependency tree for b");
        child_target(&doc, b, "source_b", Some("Add"));

        let c_id = a_c
            .target_id
            .as_ref()
            .expect("missing c reference target id");
        let c = doc
            .dependency_trees
            .get(c_id)
            .expect("missing dependency tree for c");
        child_target(&doc, c, "source_c", Some("Add"));
    }

    #[test]
    fn struct_argument_access_dependencies_keep_full_path_label() {
        let source = r#"
struct Bar {
    x: f32,
}

struct Foo {
    bar: Bar,
}

struct Input {
    foo: Foo,
}

fn use_value(v: f32) -> f32 {
    return v;
}

fn debug_main(in: Input) -> f32 {
    let a = use_value(in.foo.bar.x);
    return a;
}
"#;

        let doc = PassDebugSource::from_wgsl("path.pass", source);
        assert!(
            doc.parse_error.is_none(),
            "parse error: {:?}",
            doc.parse_error
        );

        let a_id = target_id_by_name(&doc, "a");
        let a = doc
            .dependency_trees
            .get(&a_id)
            .expect("missing dependency tree for a");
        let input = child_target(&doc, a, "in", Some("use_value"));

        assert_eq!(input.display_label.as_deref(), Some("in.foo.bar.x"));
        let input_range = input
            .source_range
            .expect("expected source range for full access path");
        assert_eq!(
            &doc.module_source[input_range.start_byte..input_range.end_byte],
            "in.foo.bar.x"
        );
    }

    #[test]
    fn sdf_bevel_depth_dependency_labels_show_local_px_access_path() {
        let source = r#"
struct GraphInputs {
    float_input_10: vec4f,
    float_input_12: vec4f,
}

@group(0) @binding(0)
var<uniform> graph_inputs: GraphInputs;

struct VSOut {
    @location(2) local_px: vec3f,
    @location(3) geo_size_px: vec2f,
}

fn sdf2d_round_rect(p: vec2f, b: vec2f, rad4: vec4f) -> f32 {
    return p.x + b.x + rad4.x;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) f32 {
    let _2d_sdf_bevel_depth_sdf_depth = sdf2d_round_rect(
        (in.local_px.xy - (in.geo_size_px * vec2f((graph_inputs.float_input_10).x))),
        (in.geo_size_px * 0.5),
        vec4f((graph_inputs.float_input_12).x),
    );
    return _2d_sdf_bevel_depth_sdf_depth;
}
"#;

        let doc = PassDebugSource::from_wgsl("sdf.pass", source);
        assert!(
            doc.parse_error.is_none(),
            "parse error: {:?}",
            doc.parse_error
        );

        let labels = dependency_labels_for(&doc, "_2d_sdf_bevel_depth_sdf_depth");
        assert_labels_contain(&labels, "in.local_px.xy");
        assert_labels_contain(&labels, "in.geo_size_px");
        assert_labels_contain(&labels, "graph_inputs.float_input_10.x");
        assert_labels_contain(&labels, "graph_inputs.float_input_12.x");
    }

    #[test]
    fn texture_sample_dependencies_include_image_sampler_and_coordinate() {
        let source = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let color = textureSample(tex, samp, uv);
    return color;
}
"#;

        let doc = PassDebugSource::from_wgsl("sample.pass", source);
        assert!(doc.parse_error.is_none());

        let color_id = target_id_by_name(&doc, "color");
        let color = doc
            .dependency_trees
            .get(&color_id)
            .expect("missing dependency tree for color");
        child_target(&doc, color, "tex", Some("textureSample"));
        child_target(&doc, color, "samp", Some("textureSample"));
        child_target(&doc, color, "uv", Some("textureSample"));
    }

    #[test]
    fn repeated_self_references_follow_previous_definitions() {
        let source = r#"
fn foo(v: f32) -> f32 {
    return v;
}

@fragment
fn fs_main() -> @location(0) f32 {
    var x: f32 = 0.0;
    x = foo(x);
    x = foo(x);
    return x;
}
"#;

        let doc = PassDebugSource::from_wgsl("reassign.pass", source);
        assert!(doc.parse_error.is_none());

        let labels = dependency_labels_for(&doc, "x");
        assert_labels_do_not_contain(&labels, "[cycle]");

        let x_id = target_id_by_name(&doc, "x");
        let x_tree = doc
            .dependency_trees
            .get(&x_id)
            .expect("missing dependency tree for x");
        let latest_x = child_target(&doc, x_tree, "x", None);
        let previous_x = child_target(&doc, latest_x, "x", Some("foo"));
        let initial_x = child_target(&doc, previous_x, "x", Some("foo"));

        let latest_start = doc.module_source.rfind("x = foo(x);").unwrap();
        assert_node_range_selects(&doc, latest_x, latest_start, "x");

        let previous_start = doc.module_source.find("x = foo(x);").unwrap();
        let latest_reference_start = latest_start + "x = foo(".len();
        assert_node_range_selects(&doc, previous_x, latest_reference_start, "x");

        let previous_reference_start = previous_start + "x = foo(".len();
        assert_node_range_selects(&doc, initial_x, previous_reference_start, "x");
    }
}
