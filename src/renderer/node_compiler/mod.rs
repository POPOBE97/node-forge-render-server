//! Node compiler infrastructure and trait definition.

pub mod input_nodes;
pub mod math_nodes;
pub mod attribute;
pub mod texture_nodes;
pub mod trigonometry_nodes;
pub mod legacy_nodes;
pub mod vector_nodes;
pub mod color_nodes;

use std::collections::HashMap;
use anyhow::Result;

use crate::dsl::{Node, SceneDSL};
use super::types::{TypedExpr, MaterialCompileContext};

/// Trait for compiling individual node types to WGSL expressions.
pub trait NodeCompiler {
    /// Compile a node to a typed WGSL expression.
    ///
    /// # Arguments
    /// * `scene` - The complete scene containing all nodes and connections
    /// * `nodes_by_id` - Map from node ID to node for fast lookup
    /// * `node` - The node to compile
    /// * `out_port` - Optional output port name (defaults to "value")
    /// * `ctx` - Compilation context for tracking resources
    /// * `cache` - Expression cache to avoid recompilation
    /// * `compile_fn` - Recursive compilation function for compiling connected nodes
    ///
    /// # Returns
    /// A `TypedExpr` containing the WGSL expression and type information
    fn compile(
        &self,
        scene: &SceneDSL,
        nodes_by_id: &HashMap<String, Node>,
        node: &Node,
        out_port: Option<&str>,
        ctx: &mut MaterialCompileContext,
        cache: &mut HashMap<(String, String), TypedExpr>,
        compile_fn: &dyn Fn(
            &SceneDSL,
            &HashMap<String, Node>,
            &str,
            Option<&str>,
            &mut MaterialCompileContext,
            &mut HashMap<(String, String), TypedExpr>,
        ) -> Result<TypedExpr>,
    ) -> Result<TypedExpr>;
}

/// Registry of node compilers by node type.
pub struct NodeCompilerRegistry {
    compilers: HashMap<String, Box<dyn NodeCompiler + Send + Sync>>,
}

impl NodeCompilerRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            compilers: HashMap::new(),
        }
    }

    /// Register a compiler for a node type.
    pub fn register(&mut self, node_type: impl Into<String>, compiler: Box<dyn NodeCompiler + Send + Sync>) {
        self.compilers.insert(node_type.into(), compiler);
    }

    /// Get a compiler for a node type.
    pub fn get(&self, node_type: &str) -> Option<&(dyn NodeCompiler + Send + Sync)> {
        self.compilers.get(node_type).map(|b| b.as_ref())
    }

    /// Create a registry with all default node compilers registered.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        
        // Register all node compilers here
        // This will be populated as we create the individual compiler modules
        
        registry
    }
}

impl Default for NodeCompilerRegistry {
    fn default() -> Self {
        Self::with_defaults()
    }
}
