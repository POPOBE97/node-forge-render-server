//! Render-pass shader debug documents.
//!
//! This module keeps the UI-facing representation intentionally small: the
//! original combined WGSL module plus collapsible trees built from Naga's IR.

mod ast;
mod dependency;
mod model;
mod source_mapping;

pub use ast::module_to_ast_tree;
use dependency::build_dependency_debug;

pub use model::{
    PassDebugAstNode, PassDebugDependencyNode, PassDebugDependencyTarget, PassDebugSourceRange,
};

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugSource {
    pub pass_name: String,
    pub module_source: String,
    pub target_texture: Option<String>,
    pub target_size: Option<[u32; 2]>,
    pub ast_tree: Vec<PassDebugAstNode>,
    pub dependency_targets: Vec<PassDebugDependencyTarget>,
    pub dependency_trees: HashMap<String, PassDebugDependencyNode>,
    pub dependency_root_target_id: Option<String>,
    pub dependency_error: Option<String>,
    pub parse_error: Option<String>,
}

impl PassDebugSource {
    pub fn from_wgsl(pass_name: impl Into<String>, module_source: impl Into<String>) -> Self {
        Self::from_wgsl_with_render_target(pass_name, module_source, None, None)
    }

    pub fn from_wgsl_with_render_target(
        pass_name: impl Into<String>,
        module_source: impl Into<String>,
        target_texture: Option<String>,
        target_size: Option<[u32; 2]>,
    ) -> Self {
        let pass_name = pass_name.into();
        let module_source = module_source.into();
        match naga::front::wgsl::parse_str(&module_source) {
            Ok(module) => {
                let dependencies = build_dependency_debug(&module, &module_source);
                let ast_tree = module_to_ast_tree(&module, &module_source, &dependencies.targets);
                Self {
                    pass_name,
                    module_source,
                    target_texture,
                    target_size,
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
                target_texture,
                target_size,
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
    fn render_target_metadata_is_preserved() {
        let source = "@fragment fn fs_main() -> @location(0) vec4f { return vec4f(1.0); }";

        let doc = PassDebugSource::from_wgsl_with_render_target(
            "target.pass",
            source,
            Some("sys.output.rt".to_string()),
            Some([640, 360]),
        );

        assert_eq!(doc.target_texture.as_deref(), Some("sys.output.rt"));
        assert_eq!(doc.target_size, Some([640, 360]));
        assert!(doc.parse_error.is_none());
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
                .join("fixtures")
                .join("render")
                .join("editor-examples")
                .join("glass")
                .join("expected")
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
