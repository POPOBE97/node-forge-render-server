fn nf_env_unit(v: vec3f) -> vec3f {
    return select(vec3f(0.0, 0.0, 1.0), v * inverseSqrt(max(dot(v, v), 1e-12)), dot(v, v) > 1e-12);
}

fn nf_env_normal(world: vec3f, local: vec2f, normal: vec3f, strength: f32) -> vec3f {
    let qx = dpdx(world); let qy = dpdy(world);
    let sx = dpdx(local); let sy = dpdy(local);
    let det = sx.x * sy.y - sx.y * sy.x;
    let safe_det = select(-max(abs(det), 1e-8), max(abs(det), 1e-8), det >= 0.0);
    let tangent = (qx * sy.y - qy * sx.y) / safe_det;
    let bitangent = (qy * sx.x - qx * sy.x) / safe_det;
    let geometric = nf_env_unit(cross(tangent, bitangent));
    return nf_env_unit(cross(bitangent, geometric) * normal.x * strength
        + cross(geometric, tangent) * normal.y * strength
        + cross(tangent, bitangent) * normal.z);
}

// Equirectangular LocalUV: +X is u=.5, +Z is u=.75, +Y is v=1.
fn nf_env_uv(direction: vec3f, rotation: f32) -> vec2f {
    let c = cos(rotation); let s = sin(rotation);
    let d = nf_env_unit(vec3f(c * direction.x - s * direction.z, direction.y,
        s * direction.x + c * direction.z));
    let u = atan2(d.z, d.x) / 6.28318530718 + 0.5;
    let v = asin(clamp(d.y, -1.0, 1.0)) / 3.14159265359 + 0.5;
    return vec2f(u, v);
}

// Manual addressing/filtering supports float32 HDR without requiring the
// optional float32-filterable GPU feature. U and V are independent.
fn nf_env_address(index: i32, size: i32, mode: i32) -> i32 {
    if (mode == 1) { return ((index % size) + size) % size; }
    if (mode == 2) {
        let period = size * 2;
        let wrapped = ((index % period) + period) % period;
        return select(period - 1 - wrapped, wrapped, wrapped < size);
    }
    return clamp(index, 0, size - 1);
}

fn nf_env_sample(image: texture_2d<f32>, direction: vec3f, rotation: f32,
    address: vec2i, linear_filter: bool) -> vec3f {
    let uv = nf_env_uv(direction, rotation);
    let size = vec2i(textureDimensions(image));
    // File storage rows are top-down; only this private boundary uses them.
    let pixel = vec2f(uv.x, 1.0 - uv.y) * vec2f(size);
    if (!linear_filter) {
        let nearest = vec2i(floor(pixel));
        return textureLoad(image, vec2i(nf_env_address(nearest.x, size.x, address.x),
            nf_env_address(nearest.y, size.y, address.y)), 0).rgb;
    }
    let p = pixel - 0.5;
    let base = vec2i(floor(p));
    let f = fract(p);
    let x0 = nf_env_address(base.x, size.x, address.x);
    let x1 = nf_env_address(base.x + 1, size.x, address.x);
    let y0 = nf_env_address(base.y, size.y, address.y);
    let y1 = nf_env_address(base.y + 1, size.y, address.y);
    return mix(
        mix(textureLoad(image, vec2i(x0, y0), 0).rgb, textureLoad(image, vec2i(x1, y0), 0).rgb, f.x),
        mix(textureLoad(image, vec2i(x0, y1), 0).rgb, textureLoad(image, vec2i(x1, y1), 0).rgb, f.x), f.y);
}

fn nf_env_reflection(image: texture_2d<f32>, r: vec3f, roughness: f32, rotation: f32,
    address: vec2i, linear_filter: bool) -> vec3f {
    if (roughness <= 0.001) { return nf_env_sample(image, r, rotation, address, linear_filter); }
    // Deterministic GGX convolution about the reflection direction. This is a
    // finite-sample reflection approximation, not a diffuse/BRDF IBL integral.
    let up = select(vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0), abs(r.y) > 0.999);
    let tx = nf_env_unit(cross(up, r));
    let ty = cross(r, tx);
    let a2 = pow(max(roughness, 0.001), 4.0);
    var radiance = vec3f(0.0);
    var weight = 0.0;
    for (var i = 0u; i < 32u; i += 1u) {
        let xi = vec2f((f32(i) + 0.5) / 32.0, f32(reverseBits(i)) * 2.3283064365386963e-10);
        let phi = 6.28318530718 * xi.x;
        let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a2 - 1.0) * xi.y));
        let sin_theta = sqrt(max(1.0 - cos_theta * cos_theta, 0.0));
        let h = tx * (cos(phi) * sin_theta) + ty * (sin(phi) * sin_theta) + r * cos_theta;
        let l = nf_env_unit(2.0 * dot(r, h) * h - r);
        let w = max(dot(r, l), 0.0);
        radiance += nf_env_sample(image, l, rotation, address, linear_filter) * w;
        weight += w;
    }
    return radiance / max(weight, 1e-6);
}

fn nf_env_material(image: texture_2d<f32>, address: vec2i, filters: vec2i, world: vec3f, local: vec2f,
    local_normal: vec3f, normal_strength: f32, view: vec3f, roughness: f32,
    rotation: f32, exposure: f32, intensity: f32, tint: vec4f, alpha: f32) -> vec4f {
    let n = nf_env_normal(world, local, local_normal, normal_strength);
    let r = nf_env_unit(reflect(-nf_env_unit(view), n));
    let angle = radians(rotation);
    let uv = nf_env_uv(r, angle);
    let dx = dpdx(uv); let dy = dpdy(uv);
    let size = vec2f(textureDimensions(image));
    // The panorama's U seam must not appear to be a full-texture footprint.
    let footprint = max(length(vec2f(dx.x - round(dx.x), dx.y) * size),
        length(vec2f(dy.x - round(dy.x), dy.y) * size));
    let linear_filter = select(filters.x, filters.y, footprint > 1.0) == 1;
    let radiance = nf_env_reflection(image, r, clamp(roughness, 0.0, 1.0), angle, address, linear_filter);
    let color = tint.rgb / max(tint.a, 1e-8);
    let coverage = clamp(alpha * tint.a, 0.0, 1.0);
    return vec4f(radiance * color * exp2(clamp(exposure, -32.0, 32.0))
        * max(intensity, 0.0) * coverage, coverage);
}
