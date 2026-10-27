// Private atlas rows are top-down. Public positions and distances remain LocalPx/Y-up.
// Header + instance records are separate from the four distance channels in each tile.
fn nf_text_record(atlas: texture_2d<f32>, index: u32) -> vec4f {
    let width = textureDimensions(atlas).x;
    return textureLoad(atlas, vec2i(i32(index % width), i32(index / width)), 0);
}

fn nf_text_distances(atlas: texture_2d<f32>, local_px: vec2f) -> vec2f {
    let header = nf_text_record(atlas, 0u);
    let position = clamp(local_px, vec2f(0.0), header.yz);
    var distances = vec2f(1e20);
    for (var i = 0u; i < u32(header.x); i += 1u) {
        let plane = nf_text_record(atlas, 1u + 2u * i);
        let outside_box = length(max(max(plane.xy - position, position - plane.zw), vec2f(0.0)));
        if (outside_box > 0.0 && outside_box > max(max(distances.x, distances.y), 0.0)) {
            continue;
        }
        let tile = nf_text_record(atlas, 2u + 2u * i);
        let scale = (plane.z - plane.x) / tile.z;
        let raster = vec2f(position.x - plane.x, plane.w - position.y) / scale - 0.5;
        let bounded = clamp(raster, vec2f(0.0), tile.zw - 1.0);
        let lo = vec2i(floor(bounded));
        let hi = min(lo + 1, vec2i(tile.zw) - 1);
        let origin = vec2i(tile.xy);
        let t = fract(bounded);
        let a = mix(textureLoad(atlas, origin + lo, 0), textureLoad(atlas, origin + vec2i(hi.x, lo.y), 0), t.x);
        let b = mix(textureLoad(atlas, origin + vec2i(lo.x, hi.y), 0), textureLoad(atlas, origin + hi, 0), t.x);
        let value = mix(a, b, t.y) * scale;
        let median = max(min(value.r, value.g), min(max(value.r, value.g), value.b));
        var glyph = vec2f(value.a, median);
        let outside = length(raster - bounded) * scale;
        if (outside > 0.0) {
            glyph = max(glyph, vec2f(0.0)) + outside;
        }
        distances = min(distances, glyph);
    }
    let outside_layout = length(local_px - position);
    if (outside_layout > 0.0) {
        distances = max(distances, vec2f(0.0)) + outside_layout;
    }
    // The atlas includes complete outlines; intersect with the authored layout frame.
    // This also gives a continuous distance at a frame edge that cuts through ink.
    let box_delta = abs(local_px - header.yz * 0.5) - header.yz * 0.5;
    let clip_distance = length(max(box_delta, vec2f(0.0))) + min(max(box_delta.x, box_delta.y), 0.0);
    return max(distances, vec2f(clip_distance));
}

fn nf_text_sdf(atlas: texture_2d<f32>, local_px: vec2f) -> f32 {
    return nf_text_distances(atlas, local_px).x;
}

fn nf_text_msdf(atlas: texture_2d<f32>, local_px: vec2f) -> f32 {
    return nf_text_distances(atlas, local_px).y;
}

fn nf_sdf_coverage(distance: f32) -> f32 {
    let aa = max(fwidth(distance), 0.0001);
    return clamp(0.5 - distance / aa, 0.0, 1.0);
}
