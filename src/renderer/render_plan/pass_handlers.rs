use anyhow::Result;

use crate::renderer::scene_prep::PreparedScene;

use super::types::ResourcePlans;

pub trait PassPlanner {
    fn node_type(&self) -> &'static str;
    fn plan(&self, _prepared: &PreparedScene, _resources: &mut ResourcePlans) -> Result<()>;
}

pub struct RenderPassPlanner;
pub struct GaussianBlurPassPlanner;
pub struct DownsamplePassPlanner;

impl PassPlanner for RenderPassPlanner {
    fn node_type(&self) -> &'static str {
        "RenderPass"
    }

    fn plan(&self, _prepared: &PreparedScene, _resources: &mut ResourcePlans) -> Result<()> {
        Ok(())
    }
}

impl PassPlanner for GaussianBlurPassPlanner {
    fn node_type(&self) -> &'static str {
        "GuassianBlurPass"
    }

    fn plan(&self, _prepared: &PreparedScene, _resources: &mut ResourcePlans) -> Result<()> {
        Ok(())
    }
}

impl PassPlanner for DownsamplePassPlanner {
    fn node_type(&self) -> &'static str {
        "Downsample"
    }

    fn plan(&self, _prepared: &PreparedScene, _resources: &mut ResourcePlans) -> Result<()> {
        Ok(())
    }
}
