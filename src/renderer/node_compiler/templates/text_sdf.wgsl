// Private texture storage is top-down; the node boundary is bottom-left LocalPx.
// Manual bilinear sampling also works on devices without FLOAT32_FILTERABLE.
fn nf_text_sdf(field: texture_2d<f32>, local_px: vec2f) -> f32 {
    let metadata = textureLoad(field, vec2i(0), 0).gba;
    let dims = vec2i(textureDimensions(field));
    let raster = vec2f(local_px.x, metadata.y - local_px.y) * metadata.z - 0.5;
    let bounded = clamp(raster, vec2f(0.0), vec2f(dims - 1));
    let lo = vec2i(floor(bounded));
    let hi = min(lo + 1, dims - 1);
    let t = fract(bounded);
    let a = mix(textureLoad(field, lo, 0).r, textureLoad(field, vec2i(hi.x, lo.y), 0).r, t.x);
    let b = mix(textureLoad(field, vec2i(lo.x, hi.y), 0).r, textureLoad(field, hi, 0).r, t.x);
    let d = mix(a, b, t.y);
    let outside = length(raster - bounded) / metadata.z;
    return select(d, max(d, 0.0) + outside, outside > 0.0);
}
fn nf_sdf_coverage(distance: f32) -> f32 {
    let aa = max(fwidth(distance), 0.0001);
    return clamp(0.5 - distance / aa, 0.0, 1.0);
}
