use std::sync::Arc;

use image::DynamicImage;

pub(crate) fn flip_image_y_rgba8(image: Arc<DynamicImage>) -> Arc<DynamicImage> {
    // The renderer's UV convention is bottom-left origin (GL-like).
    // Most image sources are top-left origin, so we flip pixels once on upload.
    let mut rgba = image.as_ref().to_rgba8();
    image::imageops::flip_vertical_in_place(&mut rgba);
    Arc::new(DynamicImage::ImageRgba8(rgba))
}
