use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    renderer::ShaderSpaceBuilder,
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::json;

const WIDTH: usize = 480;
const HEIGHT: usize = 315;

// Minimal version of practive-gaudge-meter's C -> SDF -> bevel normal -> PBR chain.
fn scene(sample_distance: f32) -> SceneDSL {
    let edges = [
        ("font", "font", "text", "font"),
        ("text", "distance", "bevel", "sdfDistance"),
        ("bevel", "normal", "pbr", "normal"),
        ("text", "coverage", "pbr", "alpha"),
        ("geometry", "geometry", "transform", "geometry"),
        ("transform", "geometry", "draw", "geometry"),
        ("pbr", "material", "draw", "material"),
        ("draw", "pass", "composite", "pass"),
        ("target", "texture", "composite", "target"),
        ("composite", "texture", "screen", "texture"),
    ];
    serde_json::from_value(json!({
        "version": "6.0",
        "workingColorSpace": "linear-srgb",
        "metadata": {"name": "text-sdf-normal-repro"},
        "nodes": [
            {"id": "font", "type": "FontFile", "params": {"assetId": "font"}},
            {"id": "text", "type": "TextSdf", "params": {
                "text": "C", "fontSize": 200, "lineHeight": 240,
                "size": [WIDTH, HEIGHT], "align": "center", "verticalAlign": "center"
            }},
            {"id": "bevel", "type": "Sdf2DBevel", "params": {
                "curve": "smooth7", "width": 4, "cliff": 0.8,
                "sampleDistance": sample_distance
            }},
            {"id": "pbr", "type": "PBRMaterial", "params": {
                "baseColor": [0.35, 0.35, 0.35, 1],
                "ambientColor": [0.02, 0.02, 0.02, 1],
                "emission": [0, 0, 0, 1],
                "lightColor": [1, 1, 1, 1],
                "lightDirection": [5.126, -5.94, 1],
                "lightIntensity": 8, "metallic": 0.12,
                "normalStrength": 0.7, "roughness": 0.5,
                "viewDirection": [0, 0, 1], "model": "pbr"
            }},
            {"id": "geometry", "type": "Rect2DGeometry", "params": {
                "size": [WIDTH, HEIGHT], "position": [0, 0]
            }},
            {"id": "transform", "type": "TransformGeometry", "params": {
                "translate": {"x": WIDTH / 2, "y": HEIGHT as f32 / 2.0, "z": 0}
            }},
            {"id": "draw", "type": "RenderPass", "params": {"blend_preset": "premul_alpha"}},
            {"id": "target", "type": "RenderTexture", "params": {
                "width": WIDTH, "height": HEIGHT, "format": "rgba16float"
            }},
            {"id": "composite", "type": "Composite", "params": {}},
            {"id": "screen", "type": "Screen", "params": {
                "width": WIDTH, "height": HEIGHT, "outputColorSpace": "srgb"
            }}
        ],
        "connections": edges.iter().enumerate().map(|(i, (a, b, c, d))| {
            json!({"id": format!("edge.{i}"), "from": {"nodeId": a, "portId": b},
                   "to": {"nodeId": c, "portId": d}})
        }).collect::<Vec<_>>(),
        "outputs": null
    }))
    .unwrap()
}

fn render(scene: &SceneDSL, renderer: &HeadlessRenderer, assets: &AssetStore) -> Vec<f32> {
    let output = ShaderSpaceBuilder::new(renderer.device.clone(), renderer.queue.clone())
        .with_adapter(renderer.adapter.clone())
        .with_asset_store(assets.clone())
        .build(scene)
        .unwrap();
    output.shader_space.render();
    output
        .shader_space
        .read_texture_rgba16f("target")
        .unwrap()
        .channels
}

// Returns (center deviation / neighbor variation, center deviation, x, y).
// Large values flag an isolated bright/dark pixel rather than a smooth shaded edge.
fn worst_isolated_pixel(pixels: &[f32]) -> (f32, f32, usize, usize) {
    let mut worst = (0.0, 0.0, 0, 0);
    for y in 1..HEIGHT - 1 {
        for x in 1..WIDTH - 1 {
            let at = |x: usize, y: usize| &pixels[(y * WIDTH + x) * 4..][..4];
            if at(x, y)[3] < 0.99
                || [at(x - 1, y), at(x + 1, y), at(x, y - 1), at(x, y + 1)]
                    .iter()
                    .any(|p| p[3] < 0.99)
            {
                continue;
            }
            let center = at(x, y);
            let neighbors = [at(x - 1, y), at(x + 1, y), at(x, y - 1), at(x, y + 1)];
            let deviation = (0..3)
                .map(|c| {
                    let average = neighbors.iter().map(|p| p[c]).sum::<f32>() * 0.25;
                    (center[c] - average).powi(2)
                })
                .sum::<f32>()
                .sqrt();
            let horizontal = (0..3)
                .map(|c| (neighbors[0][c] - neighbors[1][c]).powi(2))
                .sum::<f32>()
                .sqrt();
            let vertical = (0..3)
                .map(|c| (neighbors[2][c] - neighbors[3][c]).powi(2))
                .sum::<f32>()
                .sqrt();
            let neighbor_delta = (horizontal + vertical) * 0.5;
            let score = deviation / (neighbor_delta + 1e-5);
            if score > worst.0 {
                worst = (score, deviation, x, y);
            }
        }
    }
    worst
}

#[test]
fn sub_texel_bevel_stencil_reproduces_isolated_normal_spikes() {
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default()).unwrap();
    let assets = AssetStore::new();
    assets.insert(
        "font",
        AssetData {
            bytes: include_bytes!("fixtures/fonts/Michroma-Regular.ttf").to_vec(),
            mime_type: "font/ttf".into(),
            original_name: "Michroma-Regular.ttf".into(),
        },
    );
    // The SDF uses 2x density, so one texel is 0.5 LocalPx. The reference C uses 0.35.
    let narrow = worst_isolated_pixel(&render(&scene(0.35), &renderer, &assets));
    let wide = worst_isolated_pixel(&render(&scene(1.0), &renderer, &assets));
    println!("sampleDistance=0.35: {narrow:?}; sampleDistance=1.0: {wide:?}");

    assert!(
        narrow.0 > 4.0,
        "reference stencil should reproduce a spike: {narrow:?}"
    );
    assert!(
        wide.0 < narrow.0 * 0.5,
        "one-local-pixel stencil should suppress the spike: narrow={narrow:?}, wide={wide:?}"
    );
}
