use super::ast::*;
use super::source_mapping::*;
use super::*;

#[derive(Default)]
pub(super) struct DependencyDebugBuild {
    pub(super) targets: Vec<PassDebugDependencyTarget>,
    pub(super) trees: HashMap<String, PassDebugDependencyNode>,
    pub(super) root_target_id: Option<String>,
    pub(super) error: Option<String>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(super) enum ExprRef {
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
pub(super) type DefinitionEnv = HashMap<String, Vec<DefinitionId>>;

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

pub(super) type AccessProjection = Vec<u32>;

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

pub(super) fn build_dependency_debug(module: &Module, source: &str) -> DependencyDebugBuild {
    DependencyAnalyzer::new(module, source).into_debug()
}
