use anyhow::{Result, bail};

use crate::{
    dsl::Node,
    renderer::render_plan::pass_assemblers::{
        self,
        args::{BuilderState, SceneContext},
    },
};

pub(crate) trait PassPlanner {
    fn node_type(&self) -> &'static str;
    fn plan(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()>;
}

struct RenderPassPlanner;
struct BloomPassPlanner;
struct GaussianBlurPassPlanner;
struct GradientBlurPlanner;
struct DownsamplePassPlanner;
struct UpsamplePassPlanner;
struct CompositePassPlanner;

impl PassPlanner for RenderPassPlanner {
    fn node_type(&self) -> &'static str {
        "RenderPass"
    }

    fn plan(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()> {
        pass_assemblers::render_pass::assemble_render_pass(scene_ref, ctx, layer_id, layer_node)
    }
}

impl PassPlanner for BloomPassPlanner {
    fn node_type(&self) -> &'static str {
        "BloomNode"
    }

    fn plan(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()> {
        pass_assemblers::bloom::assemble_bloom(scene_ref, ctx, layer_id, layer_node)
    }
}

impl PassPlanner for GaussianBlurPassPlanner {
    fn node_type(&self) -> &'static str {
        "GuassianBlurPass"
    }

    fn plan(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()> {
        pass_assemblers::gaussian_blur::assemble_gaussian_blur(scene_ref, ctx, layer_id, layer_node)
    }
}

impl PassPlanner for GradientBlurPlanner {
    fn node_type(&self) -> &'static str {
        "GradientBlur"
    }

    fn plan(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()> {
        pass_assemblers::gradient_blur::assemble_gradient_blur(scene_ref, ctx, layer_id, layer_node)
    }
}

impl PassPlanner for DownsamplePassPlanner {
    fn node_type(&self) -> &'static str {
        "Downsample"
    }

    fn plan(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()> {
        pass_assemblers::downsample::assemble_downsample(scene_ref, ctx, layer_id, layer_node)
    }
}

impl PassPlanner for UpsamplePassPlanner {
    fn node_type(&self) -> &'static str {
        "Upsample"
    }

    fn plan(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()> {
        pass_assemblers::upsample::assemble_upsample(scene_ref, ctx, layer_id, layer_node)
    }
}

impl PassPlanner for CompositePassPlanner {
    fn node_type(&self) -> &'static str {
        "Composite"
    }

    fn plan(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()> {
        pass_assemblers::composite::assemble_composite(scene_ref, ctx, layer_id, layer_node)
    }
}

pub(crate) struct PassPlannerRegistry {
    planners: Vec<Box<dyn PassPlanner + Send + Sync>>,
}

impl Default for PassPlannerRegistry {
    fn default() -> Self {
        Self {
            planners: vec![
                Box::new(RenderPassPlanner),
                Box::new(BloomPassPlanner),
                Box::new(GaussianBlurPassPlanner),
                Box::new(GradientBlurPlanner),
                Box::new(DownsamplePassPlanner),
                Box::new(UpsamplePassPlanner),
                Box::new(CompositePassPlanner),
            ],
        }
    }
}

impl PassPlannerRegistry {
    pub(crate) fn plan_layer(
        &self,
        scene_ref: &SceneContext<'_>,
        ctx: &mut BuilderState<'_>,
        layer_id: &str,
        layer_node: &Node,
    ) -> Result<()> {
        let Some(planner) = self
            .planners
            .iter()
            .find(|planner| planner.node_type() == layer_node.node_type)
        else {
            bail!(
                "Composite layer must be a pass node (RenderPass/GuassianBlurPass/Downsample/Upsample/GradientBlur/Composite/BloomNode), got {} for {}. \
                 To enable chain support for new pass types, update the pass planner registry.",
                layer_node.node_type,
                layer_id
            );
        };

        planner.plan(scene_ref, ctx, layer_id, layer_node)
    }
}
