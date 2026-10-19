//! A Composite executes its ordered draws into an explicit RenderTexture.
//! Its only output is the resulting texture; drawing it again requires ImagePass.

use anyhow::{Result, bail};
use crate::dsl::{find_node, parse_texture_format};
use super::args::{BuilderState, SceneContext};

pub(crate) fn assemble_composite(
    sc: &SceneContext<'_>,
    bs: &mut BuilderState<'_>,
    layer_id: &str,
    _layer_node: &crate::dsl::Node,
) -> Result<()> {
    let Some(context) = sc.composition_contexts.get(layer_id) else {
        bail!("missing resolved Composite context for '{layer_id}'");
    };
    let target = find_node(sc.nodes_by_id(), &context.target_texture_node_id)?;
    bs.pass_output_registry.register_ports(
        layer_id,
        &["texture"],
        context.target_texture_name.clone(),
        context.target_size_px.map(|value| value.max(1.0).round() as u32),
        parse_texture_format(&target.params)?,
    );
    Ok(())
}
