use anyhow::Result;

use crate::renderer::scene_prep::PreparedScene;

use super::types::{PlanBuildOptions, RenderPlan, ResourcePlans};

pub struct RenderPlanner {
    options: PlanBuildOptions,
}

impl RenderPlanner {
    pub fn new(options: PlanBuildOptions) -> Self {
        Self { options }
    }

    pub fn plan(&self, prepared: &PreparedScene) -> Result<RenderPlan> {
        let scene_output = prepared.output_texture_name.clone();
        Ok(RenderPlan {
            resolution: prepared.resolution,
            scene_output_texture: scene_output.clone(),
            present_output_texture: scene_output,
            resources: ResourcePlans::default(),
            debug_dump_wgsl_dir: self.options.debug_dump_wgsl_dir.clone(),
        })
    }
}
