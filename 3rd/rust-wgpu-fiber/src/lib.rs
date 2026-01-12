// Re-export eframe and wgpu for convenience
pub use eframe;
pub use wgpu;

// Core types
pub mod pool;
pub mod shader_space;
pub mod composition;

// ResourceName type - used throughout the library
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceName(Arc<str>);

impl ResourceName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for ResourceName {
    fn from(s: String) -> Self {
        ResourceName(Arc::from(s.as_str()))
    }
}

impl From<&str> for ResourceName {
    fn from(s: &str) -> Self {
        ResourceName(Arc::from(s))
    }
}

impl AsRef<str> for ResourceName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ResourceName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
