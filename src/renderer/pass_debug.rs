//! Render-pass shader debug documents.
//!
//! This module keeps the UI-facing representation intentionally small: the
//! original combined WGSL module plus collapsible trees built from Naga's IR.

use std::collections::{HashMap, HashSet};

use naga::{Arena, Block, Expression, Function, Module, Statement};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassDebugAstNode {
    pub label: String,
    pub target_id: Option<String>,
    pub role: Option<String>,
    pub children: Vec<PassDebugAstNode>,
}

impl PassDebugAstNode {
    fn leaf(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            target_id: None,
            role: None,
            children: Vec::new(),
        }
    }

    fn branch(label: impl Into<String>, children: Vec<PassDebugAstNode>) -> Self {
        Self {
            label: label.into(),
            target_id: None,
            role: None,
            children,
        }
    }

    fn with_target(mut self, target_id: impl Into<String>, role: impl Into<String>) -> Self {
        self.target_id = Some(target_id.into());
        self.role = Some(role.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassDebugDependencyTarget {
    pub id: String,
    pub name: String,
    pub label: String,
    pub scope: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassDebugDependencyNode {
    pub label: String,
    pub target_id: Option<String>,
    pub children: Vec<PassDebugDependencyNode>,
}

impl PassDebugDependencyNode {
    fn leaf(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            target_id: None,
            children: Vec::new(),
        }
    }

    fn branch(label: impl Into<String>, children: Vec<PassDebugDependencyNode>) -> Self {
        Self {
            label: label.into(),
            target_id: None,
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
            target_id: Some(target_id.into()),
            children,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassDebugSource {
    pub pass_name: String,
    pub module_source: String,
    pub ast_tree: Vec<PassDebugAstNode>,
    pub dependency_targets: Vec<PassDebugDependencyTarget>,
    pub dependency_trees: HashMap<String, PassDebugDependencyNode>,
    pub dependency_error: Option<String>,
    pub parse_error: Option<String>,
}

impl PassDebugSource {
    pub fn from_wgsl(pass_name: impl Into<String>, module_source: impl Into<String>) -> Self {
        let pass_name = pass_name.into();
        let module_source = module_source.into();
        match naga::front::wgsl::parse_str(&module_source) {
            Ok(module) => {
                let dependencies = build_dependency_debug(&module);
                Self {
                    pass_name,
                    module_source,
                    ast_tree: module_to_ast_tree(&module),
                    dependency_targets: dependencies.targets,
                    dependency_trees: dependencies.trees,
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
                dependency_error: None,
                parse_error: Some(error.to_string()),
            },
        }
    }
}

pub fn module_to_ast_tree(module: &Module) -> Vec<PassDebugAstNode> {
    vec![
        entry_points_node(module),
        functions_node(module),
        globals_node(module),
        types_and_constants_node(module),
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

fn entry_points_node(module: &Module) -> PassDebugAstNode {
    let children = module
        .entry_points
        .iter()
        .map(|entry| {
            let scope = entry.name.as_str();
            let mut children = function_children(scope, &entry.function);
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

fn functions_node(module: &Module) -> PassDebugAstNode {
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
                function_children(&scope, function),
            )
        })
        .collect();
    PassDebugAstNode::branch(format!("Functions ({})", module.functions.len()), children)
}

fn globals_node(module: &Module) -> PassDebugAstNode {
    let children = module
        .global_variables
        .iter()
        .map(|(handle, global)| {
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
            .with_target(target_id_global(handle), "global")
        })
        .collect();
    PassDebugAstNode::branch(
        format!("Globals ({})", module.global_variables.len()),
        children,
    )
}

fn types_and_constants_node(module: &Module) -> PassDebugAstNode {
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
            expressions_group_node("Global Expressions", &module.global_expressions),
        ],
    )
}

fn function_children(scope: &str, function: &Function) -> Vec<PassDebugAstNode> {
    vec![
        PassDebugAstNode::branch(
            format!("Arguments ({})", function.arguments.len()),
            function
                .arguments
                .iter()
                .enumerate()
                .map(|(index, arg)| {
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
                    .with_target(target_id_arg(scope, index as u32), "argument")
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
                    .with_target(target_id_local(scope, handle), "local")
                })
                .collect(),
        ),
        expressions_group_node_with_named("Expressions", scope, function),
        PassDebugAstNode::branch(
            "Body",
            block_to_nodes(&function.body, &function.expressions, 0),
        ),
    ]
}

fn expressions_group_node(label: &str, expressions: &Arena<Expression>) -> PassDebugAstNode {
    expressions_group_node_inner(label, expressions, None, &HashSet::new())
}

fn expressions_group_node_with_named(
    label: &str,
    scope: &str,
    function: &Function,
) -> PassDebugAstNode {
    let named_handles = function
        .named_expressions
        .keys()
        .copied()
        .collect::<HashSet<_>>();
    expressions_group_node_inner(label, &function.expressions, Some(scope), &named_handles)
}

fn expressions_group_node_inner(
    label: &str,
    expressions: &Arena<Expression>,
    scope: Option<&str>,
    named_handles: &HashSet<naga::Handle<Expression>>,
) -> PassDebugAstNode {
    PassDebugAstNode::branch(
        format!("{label} ({})", expressions.len()),
        expressions
            .iter()
            .map(|(handle, expr)| {
                let node = PassDebugAstNode::branch(
                    expression_label(handle, expr),
                    expression_children(expr, expressions, 0),
                );
                if let Some(scope) = scope
                    && named_handles.contains(&handle)
                {
                    node.with_target(target_id_expr(scope, handle), "let")
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
) -> Vec<PassDebugAstNode> {
    block
        .iter()
        .enumerate()
        .map(|(index, stmt)| {
            let label = format!("{index}: {}", statement_kind_label(stmt));
            PassDebugAstNode::branch(label, statement_children(stmt, expressions, depth))
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
    depth: usize,
) -> Vec<PassDebugAstNode> {
    match stmt {
        Statement::Emit(range) => range
            .clone()
            .map(|handle| expression_node(handle, expressions, depth + 1))
            .collect(),
        Statement::Block(block) => block_to_nodes(block, expressions, depth + 1),
        Statement::If {
            condition,
            accept,
            reject,
        } => vec![
            PassDebugAstNode::branch(
                "condition",
                vec![expression_node(*condition, expressions, depth + 1)],
            ),
            PassDebugAstNode::branch("accept", block_to_nodes(accept, expressions, depth + 1)),
            PassDebugAstNode::branch("reject", block_to_nodes(reject, expressions, depth + 1)),
        ],
        Statement::Switch { selector, cases } => {
            let mut children = vec![PassDebugAstNode::branch(
                "selector",
                vec![expression_node(*selector, expressions, depth + 1)],
            )];
            children.extend(cases.iter().map(|case| {
                PassDebugAstNode::branch(
                    format!("case {:?} fall_through={}", case.value, case.fall_through),
                    block_to_nodes(&case.body, expressions, depth + 1),
                )
            }));
            children
        }
        Statement::Loop {
            body,
            continuing,
            break_if,
        } => vec![
            PassDebugAstNode::branch("body", block_to_nodes(body, expressions, depth + 1)),
            PassDebugAstNode::branch(
                "continuing",
                block_to_nodes(continuing, expressions, depth + 1),
            ),
            PassDebugAstNode::branch(
                "break_if",
                break_if
                    .map(|expr| vec![expression_node(expr, expressions, depth + 1)])
                    .unwrap_or_else(|| vec![PassDebugAstNode::leaf("none")]),
            ),
        ],
        Statement::Return { value } => value
            .map(|expr| vec![expression_node(expr, expressions, depth + 1)])
            .unwrap_or_default(),
        Statement::Store { pointer, value } => vec![
            PassDebugAstNode::branch(
                "pointer",
                vec![expression_node(*pointer, expressions, depth + 1)],
            ),
            PassDebugAstNode::branch(
                "value",
                vec![expression_node(*value, expressions, depth + 1)],
            ),
        ],
        Statement::ImageStore {
            image,
            coordinate,
            array_index,
            value,
        } => expr_list_nodes(
            expressions,
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
            depth,
            &[
                ("pointer", Some(*pointer)),
                ("value", Some(*value)),
                ("result", Some(*result)),
            ],
        ),
        Statement::WorkGroupUniformLoad { pointer, result } => expr_list_nodes(
            expressions,
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
                        vec![expression_node(*arg, expressions, depth + 1)],
                    )
                })
                .collect();
            if let Some(result) = result {
                children.push(PassDebugAstNode::branch(
                    "result",
                    vec![expression_node(*result, expressions, depth + 1)],
                ));
            }
            children
        }
        Statement::RayQuery { query, .. } => {
            vec![PassDebugAstNode::branch(
                "query",
                vec![expression_node(*query, expressions, depth + 1)],
            )]
        }
        Statement::SubgroupBallot { result, predicate } => expr_list_nodes(
            expressions,
            depth,
            &[("result", Some(*result)), ("predicate", *predicate)],
        ),
        Statement::SubgroupGather {
            argument, result, ..
        } => expr_list_nodes(
            expressions,
            depth,
            &[("argument", Some(*argument)), ("result", Some(*result))],
        ),
        Statement::SubgroupCollectiveOperation {
            argument, result, ..
        } => expr_list_nodes(
            expressions,
            depth,
            &[("argument", Some(*argument)), ("result", Some(*result))],
        ),
        other => vec![PassDebugAstNode::leaf(format!("{other:#?}"))],
    }
}

fn expression_node(
    handle: naga::Handle<Expression>,
    expressions: &Arena<Expression>,
    depth: usize,
) -> PassDebugAstNode {
    let expr = &expressions[handle];
    PassDebugAstNode::branch(
        expression_label(handle, expr),
        expression_children(expr, expressions, depth),
    )
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
                    vec![expression_node(*component, expressions, depth + 1)],
                )
            })
            .collect(),
        Expression::Access { base, index } => expr_list_nodes(
            expressions,
            depth,
            &[("base", Some(*base)), ("index", Some(*index))],
        ),
        Expression::AccessIndex { base, .. } => {
            expr_list_nodes(expressions, depth, &[("base", Some(*base))])
        }
        Expression::Splat { value, .. } => {
            expr_list_nodes(expressions, depth, &[("value", Some(*value))])
        }
        Expression::Swizzle { vector, .. } => {
            expr_list_nodes(expressions, depth, &[("vector", Some(*vector))])
        }
        Expression::Load { pointer } => {
            expr_list_nodes(expressions, depth, &[("pointer", Some(*pointer))])
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
            expr_list_nodes(expressions, depth, &[("image", Some(*image))])
        }
        Expression::Unary { expr, .. } => {
            expr_list_nodes(expressions, depth, &[("expr", Some(*expr))])
        }
        Expression::Binary { left, right, .. } => expr_list_nodes(
            expressions,
            depth,
            &[("left", Some(*left)), ("right", Some(*right))],
        ),
        Expression::Select {
            condition,
            accept,
            reject,
        } => expr_list_nodes(
            expressions,
            depth,
            &[
                ("condition", Some(*condition)),
                ("accept", Some(*accept)),
                ("reject", Some(*reject)),
            ],
        ),
        Expression::Derivative { expr, .. } => {
            expr_list_nodes(expressions, depth, &[("expr", Some(*expr))])
        }
        Expression::Relational { argument, .. } => {
            expr_list_nodes(expressions, depth, &[("argument", Some(*argument))])
        }
        Expression::Math {
            arg,
            arg1,
            arg2,
            arg3,
            ..
        } => expr_list_nodes(
            expressions,
            depth,
            &[
                ("arg", Some(*arg)),
                ("arg1", *arg1),
                ("arg2", *arg2),
                ("arg3", *arg3),
            ],
        ),
        Expression::As { expr, .. } => {
            expr_list_nodes(expressions, depth, &[("expr", Some(*expr))])
        }
        Expression::RayQueryGetIntersection { query, .. } => {
            expr_list_nodes(expressions, depth, &[("query", Some(*query))])
        }
        other => vec![PassDebugAstNode::leaf(format!("{other:#?}"))],
    }
}

fn expr_list_nodes(
    expressions: &Arena<Expression>,
    depth: usize,
    handles: &[(&str, Option<naga::Handle<Expression>>)],
) -> Vec<PassDebugAstNode> {
    handles
        .iter()
        .filter_map(|(label, handle)| {
            handle.map(|handle| {
                PassDebugAstNode::branch(
                    *label,
                    vec![expression_node(handle, expressions, depth + 1)],
                )
            })
        })
        .collect()
}

#[derive(Default)]
struct DependencyDebugBuild {
    targets: Vec<PassDebugDependencyTarget>,
    trees: HashMap<String, PassDebugDependencyNode>,
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
}

#[derive(Clone, Debug)]
struct StoreDependency {
    scope: String,
    value: naga::Handle<Expression>,
    controls: Vec<(String, ExprRef)>,
}

#[derive(Clone, Debug)]
struct ReturnDependency {
    value: Option<naga::Handle<Expression>>,
    controls: Vec<(String, ExprRef)>,
}

#[derive(Clone, Debug)]
struct CallDependency {
    function: naga::Handle<Function>,
    arguments: Vec<naga::Handle<Expression>>,
}

struct DependencyAnalyzer<'a> {
    module: &'a Module,
    functions: HashMap<String, &'a Function>,
    function_handles: HashMap<naga::Handle<Function>, String>,
    targets: Vec<PassDebugDependencyTarget>,
    target_kinds: HashMap<String, TargetKind>,
    stores_by_target: HashMap<String, Vec<StoreDependency>>,
    returns_by_function: HashMap<String, Vec<ReturnDependency>>,
    calls_by_result: HashMap<(String, usize), CallDependency>,
}

impl<'a> DependencyAnalyzer<'a> {
    fn new(module: &'a Module) -> Self {
        let mut analyzer = Self {
            module,
            functions: HashMap::new(),
            function_handles: HashMap::new(),
            targets: Vec::new(),
            target_kinds: HashMap::new(),
            stores_by_target: HashMap::new(),
            returns_by_function: HashMap::new(),
            calls_by_result: HashMap::new(),
        };
        analyzer.index_module();
        analyzer
    }

    fn into_debug(mut self) -> DependencyDebugBuild {
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

        self.collect_block_dependencies(&scope, function, &function.body, &mut Vec::new());
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
        self.target_kinds.insert(id.clone(), target_kind);
        self.targets.push(PassDebugDependencyTarget {
            id,
            name,
            label,
            scope,
            kind,
        });
    }

    fn collect_block_dependencies(
        &mut self,
        scope: &str,
        function: &'a Function,
        block: &Block,
        controls: &mut Vec<(String, ExprRef)>,
    ) {
        for stmt in block {
            match stmt {
                Statement::Block(block) => {
                    self.collect_block_dependencies(scope, function, block, controls);
                }
                Statement::If {
                    condition,
                    accept,
                    reject,
                } => {
                    controls.push((
                        "[condition] if".to_string(),
                        ExprRef::Function {
                            scope: scope.to_string(),
                            handle: *condition,
                        },
                    ));
                    self.collect_block_dependencies(scope, function, accept, controls);
                    self.collect_block_dependencies(scope, function, reject, controls);
                    controls.pop();
                }
                Statement::Switch { selector, cases } => {
                    controls.push((
                        "[condition] switch selector".to_string(),
                        ExprRef::Function {
                            scope: scope.to_string(),
                            handle: *selector,
                        },
                    ));
                    for case in cases {
                        controls.push((
                            format!("[condition] case {:?}", case.value),
                            ExprRef::Function {
                                scope: scope.to_string(),
                                handle: *selector,
                            },
                        ));
                        self.collect_block_dependencies(scope, function, &case.body, controls);
                        controls.pop();
                    }
                    controls.pop();
                }
                Statement::Loop {
                    body,
                    continuing,
                    break_if,
                } => {
                    if let Some(expr) = break_if {
                        controls.push((
                            "[condition] loop break_if".to_string(),
                            ExprRef::Function {
                                scope: scope.to_string(),
                                handle: *expr,
                            },
                        ));
                    }
                    self.collect_block_dependencies(scope, function, body, controls);
                    self.collect_block_dependencies(scope, function, continuing, controls);
                    if break_if.is_some() {
                        controls.pop();
                    }
                }
                Statement::Store { pointer, value } => {
                    let pointer_ref = ExprRef::Function {
                        scope: scope.to_string(),
                        handle: *pointer,
                    };
                    if let Some(target_id) = self.resolve_pointer_target(&pointer_ref) {
                        self.stores_by_target
                            .entry(target_id)
                            .or_default()
                            .push(StoreDependency {
                                scope: scope.to_string(),
                                value: *value,
                                controls: controls.clone(),
                            });
                    }
                }
                Statement::Return { value } => {
                    self.returns_by_function
                        .entry(scope.to_string())
                        .or_default()
                        .push(ReturnDependency {
                            value: *value,
                            controls: controls.clone(),
                        });
                }
                Statement::Call {
                    function,
                    arguments,
                    result,
                } => {
                    if let Some(result) = result {
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
        const MAX_DEPENDENCY_DEPTH: usize = 36;
        if depth >= MAX_DEPENDENCY_DEPTH {
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
            );
        }

        let Some(target) = self.targets.iter().find(|target| target.id == target_id) else {
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
            TargetKind::NamedExpression { scope, handle } => vec![self.build_expr_dependency(
                ExprRef::Function {
                    scope: scope.clone(),
                    handle: *handle,
                },
                "[value]",
                target_stack,
                &mut HashSet::new(),
                depth + 1,
            )],
        };
        if children.is_empty() {
            children.push(PassDebugDependencyNode::leaf("[source] no contributors"));
        }
        target_stack.pop();

        PassDebugDependencyNode::target(
            format!("{} ({})", target.label, target.kind),
            target.id.clone(),
            children,
        )
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
            children.push(self.build_expr_dependency(
                ExprRef::Global(init),
                "[init]",
                target_stack,
                &mut HashSet::new(),
                depth + 1,
            ));
        }
        let target_id = target_id_global(handle);
        children.extend(self.store_nodes_for_target(&target_id, target_stack, depth + 1));
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
        let mut children = Vec::new();
        if let Some(function) = self.functions.get(scope) {
            let local = &function.local_variables[handle];
            if let Some(init) = local.init {
                children.push(self.build_expr_dependency(
                    ExprRef::Function {
                        scope: scope.to_string(),
                        handle: init,
                    },
                    "[init]",
                    target_stack,
                    &mut HashSet::new(),
                    depth + 1,
                ));
            }
        }
        children.extend(self.store_nodes_for_target(target_id, target_stack, depth + 1));
        children
    }

    fn store_nodes_for_target(
        &self,
        target_id: &str,
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        self.stores_by_target
            .get(target_id)
            .map(|stores| {
                stores
                    .iter()
                    .enumerate()
                    .map(|(index, store)| {
                        let mut children =
                            self.control_nodes(&store.controls, target_stack, depth + 1);
                        children.push(self.build_expr_dependency(
                            ExprRef::Function {
                                scope: store.scope.clone(),
                                handle: store.value,
                            },
                            "[rhs]",
                            target_stack,
                            &mut HashSet::new(),
                            depth + 1,
                        ));
                        PassDebugDependencyNode::branch(
                            format!("[store {index}] {}", store.scope),
                            children,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn control_nodes(
        &self,
        controls: &[(String, ExprRef)],
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        controls
            .iter()
            .map(|(label, expr)| {
                self.build_expr_dependency(
                    expr.clone(),
                    label,
                    target_stack,
                    &mut HashSet::new(),
                    depth + 1,
                )
            })
            .collect()
    }

    fn build_expr_dependency(
        &self,
        expr_ref: ExprRef,
        edge_label: &str,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        depth: usize,
    ) -> PassDebugDependencyNode {
        const MAX_DEPENDENCY_DEPTH: usize = 36;
        if depth >= MAX_DEPENDENCY_DEPTH {
            return PassDebugDependencyNode::leaf(format!("{edge_label} [depth limit]"));
        }

        if let Some(target_id) = self.named_expression_target_id(&expr_ref)
            && !target_stack.iter().any(|id| id == &target_id)
        {
            return PassDebugDependencyNode::branch(
                format!("{edge_label} named expression"),
                vec![self.build_target_tree(&target_id, target_stack, depth + 1)],
            );
        }

        let expr_key = expr_ref.key();
        if !seen_exprs.insert(expr_key) {
            return PassDebugDependencyNode::leaf(format!("{edge_label} already shown"));
        }

        let Some(expr) = self.expression(&expr_ref) else {
            return PassDebugDependencyNode::leaf(format!("{edge_label} missing expression"));
        };

        let mut children = self.expression_dependency_children(
            expr_ref.clone(),
            expr,
            target_stack,
            seen_exprs,
            depth + 1,
        );
        if children.is_empty() {
            children.push(PassDebugDependencyNode::leaf("[leaf]"));
        }

        PassDebugDependencyNode::branch(
            format!("{edge_label} {}", expression_kind_label(expr)),
            children,
        )
    }

    fn expression_dependency_children(
        &self,
        expr_ref: ExprRef,
        expr: &Expression,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        match expr {
            Expression::Literal(_) | Expression::ZeroValue(_) => Vec::new(),
            Expression::Constant(handle) => {
                let init = self.module.constants[*handle].init;
                vec![self.build_expr_dependency(
                    ExprRef::Global(init),
                    "[constant.init]",
                    target_stack,
                    seen_exprs,
                    depth + 1,
                )]
            }
            Expression::Override(handle) => self.module.overrides[*handle]
                .init
                .map(|init| {
                    vec![self.build_expr_dependency(
                        ExprRef::Global(init),
                        "[override.init]",
                        target_stack,
                        seen_exprs,
                        depth + 1,
                    )]
                })
                .unwrap_or_default(),
            Expression::Compose { components, .. } => components
                .iter()
                .enumerate()
                .map(|(index, component)| {
                    self.build_sibling_expr_dependency(
                        &expr_ref,
                        *component,
                        &format!("[component {index}]"),
                        target_stack,
                        seen_exprs,
                        depth + 1,
                    )
                })
                .collect(),
            Expression::Access { base, index } => vec![
                self.build_sibling_expr_dependency(
                    &expr_ref,
                    *base,
                    "[access.base]",
                    target_stack,
                    seen_exprs,
                    depth + 1,
                ),
                self.build_sibling_expr_dependency(
                    &expr_ref,
                    *index,
                    "[access.index]",
                    target_stack,
                    seen_exprs,
                    depth + 1,
                ),
            ],
            Expression::AccessIndex { base, index } => vec![self.build_sibling_expr_dependency(
                &expr_ref,
                *base,
                &format!("[access.{index}]"),
                target_stack,
                seen_exprs,
                depth + 1,
            )],
            Expression::Splat { value, .. } => vec![self.build_sibling_expr_dependency(
                &expr_ref,
                *value,
                "[splat.value]",
                target_stack,
                seen_exprs,
                depth + 1,
            )],
            Expression::Swizzle { vector, .. } => vec![self.build_sibling_expr_dependency(
                &expr_ref,
                *vector,
                "[swizzle.vector]",
                target_stack,
                seen_exprs,
                depth + 1,
            )],
            Expression::FunctionArgument(index) => self.target_node_for_expr_target(
                &expr_ref,
                target_id_arg(expr_ref_scope(&expr_ref), *index),
                target_stack,
                depth,
            ),
            Expression::GlobalVariable(handle) => self.target_node_for_expr_target(
                &expr_ref,
                target_id_global(*handle),
                target_stack,
                depth,
            ),
            Expression::LocalVariable(handle) => self.target_node_for_expr_target(
                &expr_ref,
                target_id_local(expr_ref_scope(&expr_ref), *handle),
                target_stack,
                depth,
            ),
            Expression::Load { pointer } => {
                let pointer_ref = self.sibling_expr_ref(&expr_ref, *pointer);
                let mut children = vec![self.build_expr_dependency(
                    pointer_ref.clone(),
                    "[load.pointer]",
                    target_stack,
                    seen_exprs,
                    depth + 1,
                )];
                if let Some(target_id) = self.resolve_pointer_target(&pointer_ref) {
                    children.push(self.build_target_tree(&target_id, target_stack, depth + 1));
                }
                children
            }
            Expression::ImageSample {
                image,
                sampler,
                coordinate,
                array_index,
                offset,
                depth_ref,
                ..
            } => self.expr_operand_nodes(
                &expr_ref,
                &[
                    ("[sample.image]", Some(*image)),
                    ("[sample.sampler]", Some(*sampler)),
                    ("[sample.coordinate]", Some(*coordinate)),
                    ("[sample.array_index]", *array_index),
                    ("[sample.offset]", *offset),
                    ("[sample.depth_ref]", *depth_ref),
                ],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::ImageLoad {
                image,
                coordinate,
                array_index,
                sample,
                level,
            } => self.expr_operand_nodes(
                &expr_ref,
                &[
                    ("[image_load.image]", Some(*image)),
                    ("[image_load.coordinate]", Some(*coordinate)),
                    ("[image_load.array_index]", *array_index),
                    ("[image_load.sample]", *sample),
                    ("[image_load.level]", *level),
                ],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::ImageQuery { image, .. } => self.expr_operand_nodes(
                &expr_ref,
                &[("[image_query.image]", Some(*image))],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::Unary { expr, .. } => self.expr_operand_nodes(
                &expr_ref,
                &[("[unary.expr]", Some(*expr))],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::Binary { left, right, .. } => self.expr_operand_nodes(
                &expr_ref,
                &[
                    ("[binary.left]", Some(*left)),
                    ("[binary.right]", Some(*right)),
                ],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::Select {
                condition,
                accept,
                reject,
            } => self.expr_operand_nodes(
                &expr_ref,
                &[
                    ("[select.condition]", Some(*condition)),
                    ("[select.accept]", Some(*accept)),
                    ("[select.reject]", Some(*reject)),
                ],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::Derivative { expr, .. } => self.expr_operand_nodes(
                &expr_ref,
                &[("[derivative.expr]", Some(*expr))],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::Relational { argument, .. } => self.expr_operand_nodes(
                &expr_ref,
                &[("[relational.argument]", Some(*argument))],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::Math {
                arg,
                arg1,
                arg2,
                arg3,
                ..
            } => self.expr_operand_nodes(
                &expr_ref,
                &[
                    ("[math.arg]", Some(*arg)),
                    ("[math.arg1]", *arg1),
                    ("[math.arg2]", *arg2),
                    ("[math.arg3]", *arg3),
                ],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::As { expr, .. } => self.expr_operand_nodes(
                &expr_ref,
                &[("[as.expr]", Some(*expr))],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::CallResult(_) => {
                let ExprRef::Function { scope, handle } = &expr_ref else {
                    return Vec::new();
                };
                let Some(call) = self.calls_by_result.get(&(scope.clone(), handle.index())) else {
                    return Vec::new();
                };
                let mut children = call
                    .arguments
                    .iter()
                    .enumerate()
                    .map(|(index, arg)| {
                        self.build_expr_dependency(
                            ExprRef::Function {
                                scope: scope.clone(),
                                handle: *arg,
                            },
                            &format!("[arg {index}]"),
                            target_stack,
                            seen_exprs,
                            depth + 1,
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some(callee_scope) = self.function_handles.get(&call.function) {
                    if target_stack
                        .iter()
                        .any(|target| target == &format!("return::{callee_scope}"))
                    {
                        children.push(PassDebugDependencyNode::leaf(format!(
                            "[return] {callee_scope} [cycle]"
                        )));
                    } else {
                        target_stack.push(format!("return::{callee_scope}"));
                        children.extend(self.return_nodes(callee_scope, target_stack, depth + 1));
                        target_stack.pop();
                    }
                }
                children
            }
            Expression::ArrayLength(handle) => self.expr_operand_nodes(
                &expr_ref,
                &[("[array_length.pointer]", Some(*handle))],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            Expression::RayQueryGetIntersection { query, .. } => self.expr_operand_nodes(
                &expr_ref,
                &[("[ray_query.query]", Some(*query))],
                target_stack,
                seen_exprs,
                depth + 1,
            ),
            _ => Vec::new(),
        }
    }

    fn return_nodes(
        &self,
        scope: &str,
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        self.returns_by_function
            .get(scope)
            .map(|returns| {
                returns
                    .iter()
                    .enumerate()
                    .map(|(index, ret)| {
                        let mut children =
                            self.control_nodes(&ret.controls, target_stack, depth + 1);
                        if let Some(value) = ret.value {
                            children.push(self.build_expr_dependency(
                                ExprRef::Function {
                                    scope: scope.to_string(),
                                    handle: value,
                                },
                                "[return.value]",
                                target_stack,
                                &mut HashSet::new(),
                                depth + 1,
                            ));
                        }
                        PassDebugDependencyNode::branch(
                            format!("[return {index}] {scope}"),
                            children,
                        )
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                vec![PassDebugDependencyNode::leaf(format!(
                    "[return] {scope} has no return value"
                ))]
            })
    }

    fn expr_operand_nodes(
        &self,
        current: &ExprRef,
        operands: &[(&str, Option<naga::Handle<Expression>>)],
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        operands
            .iter()
            .filter_map(|(label, handle)| {
                handle.map(|handle| {
                    self.build_sibling_expr_dependency(
                        current,
                        handle,
                        label,
                        target_stack,
                        seen_exprs,
                        depth + 1,
                    )
                })
            })
            .collect()
    }

    fn build_sibling_expr_dependency(
        &self,
        current: &ExprRef,
        handle: naga::Handle<Expression>,
        edge_label: &str,
        target_stack: &mut Vec<String>,
        seen_exprs: &mut HashSet<String>,
        depth: usize,
    ) -> PassDebugDependencyNode {
        self.build_expr_dependency(
            self.sibling_expr_ref(current, handle),
            edge_label,
            target_stack,
            seen_exprs,
            depth + 1,
        )
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

    fn target_node_for_expr_target(
        &self,
        current: &ExprRef,
        target_id: String,
        target_stack: &mut Vec<String>,
        depth: usize,
    ) -> Vec<PassDebugDependencyNode> {
        match current {
            ExprRef::Function { .. } | ExprRef::Global(_) => {
                vec![self.build_target_tree(&target_id, target_stack, depth + 1)]
            }
        }
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

fn build_dependency_debug(module: &Module) -> DependencyDebugBuild {
    DependencyAnalyzer::new(module).into_debug()
}

#[cfg(test)]
mod tests {
    use super::{PassDebugDependencyNode, PassDebugSource};

    fn target_id_by_name(doc: &PassDebugSource, name: &str) -> String {
        doc.dependency_targets
            .iter()
            .find(|target| target.name == name)
            .unwrap_or_else(|| panic!("missing dependency target named {name}"))
            .id
            .clone()
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
        assert_labels_contain(&x_labels, "[init]");
        assert_labels_contain(&x_labels, "[store 0]");
        assert_labels_contain(&x_labels, "[rhs]");
        assert_labels_contain(&x_labels, "fs_main let y");
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
    fn function_call_result_includes_arguments_and_callee_return() {
        let source = r#"
fn helper(a: f32, gate: bool) -> f32 {
    var out: f32 = 0.0;
    if gate {
        out = a + 1.0;
    }
    return out;
}

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
    var x: f32 = 0.0;
    let y = uv.x;
    x = helper(y, true);
    return vec4f(x, x, x, 1.0);
}
"#;

        let doc = PassDebugSource::from_wgsl("call.pass", source);
        assert!(doc.parse_error.is_none());

        let labels = dependency_labels_for(&doc, "x");
        assert_labels_contain(&labels, "[arg 0]");
        assert_labels_contain(&labels, "[arg 1]");
        assert_labels_contain(&labels, "[return 0] helper");
        assert_labels_contain(&labels, "helper local out");
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

        let labels = dependency_labels_for(&doc, "color");
        assert_labels_contain(&labels, "[sample.image]");
        assert_labels_contain(&labels, "[sample.sampler]");
        assert_labels_contain(&labels, "[sample.coordinate]");
        assert_labels_contain(&labels, "global tex");
        assert_labels_contain(&labels, "global samp");
        assert_labels_contain(&labels, "argument uv");
    }

    #[test]
    fn repeated_self_references_stop_with_cycle_marker() {
        let source = r#"
@fragment
fn fs_main() -> @location(0) vec4f {
    var x: f32 = 0.0;
    x = x + 1.0;
    return vec4f(x, x, x, 1.0);
}
"#;

        let doc = PassDebugSource::from_wgsl("cycle.pass", source);
        assert!(doc.parse_error.is_none());

        let labels = dependency_labels_for(&doc, "x");
        assert_labels_contain(&labels, "[cycle]");
    }
}
