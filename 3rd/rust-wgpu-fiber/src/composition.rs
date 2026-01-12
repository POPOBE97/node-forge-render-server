use crate::ResourceName;

pub struct CompositionBuilder {
    passes: Vec<ResourceName>,
}

impl CompositionBuilder {
    pub fn new() -> Self {
        CompositionBuilder { passes: Vec::new() }
    }

    pub fn pass(mut self, name: ResourceName) -> Self {
        self.passes.push(name);
        self
    }

    pub fn pass_with_deps<F>(mut self, name: ResourceName, deps_builder: F) -> Self
    where
        F: FnOnce(CompositionBuilder) -> CompositionBuilder,
    {
        let deps = deps_builder(CompositionBuilder::new());
        self.passes.extend(deps.passes);
        self.passes.push(name);
        self
    }

    pub(crate) fn build(self) -> Vec<ResourceName> {
        self.passes
    }
}

impl Default for CompositionBuilder {
    fn default() -> Self {
        Self::new()
    }
}
