struct NfWave {
    height: f32,
    dir: vec2f,
    normal: vec3f,
};

// All spatial values belong to the consuming geometry, in bottom-left/Y-up LocalPx.
// Progress is already authored time: looping and time scaling belong upstream.
fn nf_wave(
    coord: vec2f,
    geometry_height: f32,
    progress: f32,
    origin: vec2f,
    amplitude: f32,
    frequency: f32,
    decay: f32,
    speed: f32,
    damping: f32,
    front_feather: f32,
) -> NfWave {
    let scale = max(geometry_height, 1.0);
    let delta = coord - origin;
    let radius = length(delta);
    let dir = delta / max(radius, 0.000001);
    let speed_px = max(speed * scale, 0.001);
    let damping_px = damping / scale;
    let t = progress - radius / speed_px;
    if (t <= 0.0) {
        return NfWave(0.0, dir, vec3f(0.0, 0.0, 1.0));
    }

    let phase = frequency * t;
    let attenuation = amplitude * scale * exp(-decay * t - radius * damping_px);
    let wave = sin(phase);
    var onset = 1.0;
    var onset_dr = 0.0;
    if (front_feather > 0.0) {
        let x = clamp(t * speed_px / front_feather, 0.0, 1.0);
        onset = x * x * x * (10.0 + x * (-15.0 + 6.0 * x));
        onset_dr = -30.0 * x * x * (1.0 - x) * (1.0 - x) / front_feather;
    }
    let height = attenuation * wave * onset;
    // d(height)/d(radius), including both attenuation and the feather envelope.
    let slope = attenuation * (
        ((decay / speed_px - damping_px) * wave - frequency / speed_px * cos(phase)) * onset
        + wave * onset_dr
    );
    // The radial direction is zero at the center, giving a finite +Z normal there.
    let normal = normalize(vec3f(-slope * dir, 1.0));
    return NfWave(height, dir, normal);
}
