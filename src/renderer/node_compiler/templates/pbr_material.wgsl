fn nf_pbr_unit(v: vec3f) -> vec3f {
    return select(vec3f(0.0, 0.0, 1.0), v * inverseSqrt(max(dot(v, v), 1e-12)), dot(v, v) > 1e-12);
}
fn nf_pbr(world: vec3f, local: vec2f, base: vec4f, metallic_in: f32,
    roughness_in: f32, local_normal: vec3f, normal_strength: f32, emission: vec4f,
    alpha: f32, light_direction: vec3f, light_color: vec4f, intensity: f32,
    ambient: vec4f, view_direction: vec3f) -> vec4f {
    // Derivative frame follows each instance's rotation, scale and reflection.
    let qx = dpdx(world); let qy = dpdy(world);
    let sx = dpdx(local); let sy = dpdy(local);
    let det = sx.x * sy.y - sx.y * sy.x;
    let safe_det = select(-max(abs(det), 1e-8), max(abs(det), 1e-8), det >= 0.0);
    let tangent = (qx * sy.y - qy * sx.y) / safe_det;
    let bitangent = (qy * sx.x - qx * sy.x) / safe_det;
    let geometric = nf_pbr_unit(cross(tangent, bitangent));
    // Inverse-transpose of the local frame, so a local SDF normal stays local.
    let n = nf_pbr_unit(cross(bitangent, geometric) * local_normal.x * normal_strength
        + cross(geometric, tangent) * local_normal.y * normal_strength
        + cross(tangent, bitangent) * local_normal.z);
    let l = nf_pbr_unit(light_direction);
    let v = nf_pbr_unit(view_direction);
    let h = nf_pbr_unit(l + v);
    let nl = max(dot(n, l), 0.0);
    let nv = max(dot(n, v), 0.0001);
    let nh = max(dot(n, h), 0.0);
    let vh = max(dot(v, h), 0.0);
    let metallic = clamp(metallic_in, 0.0, 1.0);
    let roughness = clamp(roughness_in, 0.045, 1.0);
    let color = max(base.rgb / max(base.a, 1e-8), vec3f(0.0));
    let f0 = mix(vec3f(0.04), color, metallic);
    let fresnel = f0 + (1.0 - f0) * pow(1.0 - vh, 5.0);
    let a2 = pow(roughness, 4.0);
    let denom = nh * nh * (a2 - 1.0) + 1.0;
    let distribution = a2 / max(3.14159265 * denom * denom, 1e-8);
    let visibility = 0.5 / max(nl * sqrt(nv * nv * (1.0 - a2) + a2)
        + nv * sqrt(nl * nl * (1.0 - a2) + a2), 1e-6);
    let diffuse = (1.0 - fresnel) * (1.0 - metallic) * color / 3.14159265;
    let specular = distribution * visibility * fresnel;
    let radiance = light_color.rgb * max(intensity, 0.0);
    let lit = (diffuse + specular) * radiance * nl
        + ambient.rgb * color * (1.0 - metallic) + emission.rgb;
    let coverage = clamp(alpha * base.a, 0.0, 1.0);
    return vec4f(lit * coverage, coverage);
}
