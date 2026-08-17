//! Formal renderer contracts for nodes backed by the Gaussian processing pipeline.

pub(crate) const GAUSSIAN_BLUR_RADIUS_PORT: &str = "radius";
pub(crate) const IMAGE_PASS_BLUR_RADIUS_PORT: &str = "blurRadius";

pub(crate) fn gaussian_radius_port_id(node_type: &str) -> Option<&'static str> {
    match node_type {
        "GuassianBlurPass" => Some(GAUSSIAN_BLUR_RADIUS_PORT),
        "ImagePass" => Some(IMAGE_PASS_BLUR_RADIUS_PORT),
        _ => None,
    }
}
