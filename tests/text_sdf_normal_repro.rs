use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    renderer::ShaderSpaceBuilder,
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::json;

const WIDTH: usize = 480;
const HEIGHT: usize = 315;

// Minimal version of practice-gaudge-meter's C -> SDF -> bevel normal -> PBR chain.
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
        "version": "6.1",
        "workingColorSpace": "linear-srgb", "dpi": 3.0,
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

#[test]
fn mtsdf_bevel_stencils_preserve_coverage_and_converge() {
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
    // A fixed-em true-distance tile supports subpixel samples at any display scale.
    // Changing the normal stencil must not change the independent RGB coverage.
    let narrow = render(&scene(0.35), &renderer, &assets);
    let wide = render(&scene(1.0), &renderer, &assets);
    assert!(narrow.iter().chain(&wide).all(|v| v.is_finite()));
    let mut energy = 0.0;
    let mut difference = 0.0;
    let mut ink = 0;
    for (a, b) in narrow.chunks_exact(4).zip(wide.chunks_exact(4)) {
        assert!((a[3] - b[3]).abs() < 0.001, "normal sampling changed glyph coverage");
        if a[3] > 0.99 { ink += 1; }
        for c in 0..3 {
            energy += a[c].abs();
            difference += (a[c] - b[c]).abs();
        }
    }
    assert!(ink > 1000, "missing glyph body");
    assert!(difference / energy.max(1.0) < 0.08,
        "subpixel stencils should produce consistent shading: difference={difference}, energy={energy}");
}
