use std::io::Cursor;

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, Rgba};
use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    renderer::ShaderSpaceBuilder,
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::{Value, json};

fn scene() -> SceneDSL {
    let edges = [
        ("env", "material", "draw", "material"),
        ("geometry", "geometry", "transform", "geometry"),
        ("transform", "geometry", "draw", "geometry"),
        ("draw", "pass", "composite", "pass"),
        ("target", "texture", "composite", "target"),
        ("composite", "texture", "screen", "texture"),
    ];
    serde_json::from_value(json!({
        "version":"6.1", "workingColorSpace":"linear-srgb", "dpi":1,
        "metadata":{"name":"HDRI environment"},
        "nodes":[
            {"id":"env", "type":"EnvMaterial", "params":{"assetId":"env"}},
            {"id":"geometry", "type":"Rect2DGeometry", "params":{"size":[16,16],"position":[0,0]}},
            {"id":"transform", "type":"TransformGeometry", "params":{"translate":{"x":8,"y":8,"z":0},"rotate":{"x":0,"y":0,"z":0}}},
            {"id":"draw", "type":"RenderPass"},
            {"id":"target", "type":"RenderTexture", "params":{"width":16,"height":16,"format":"rgba16float"}},
            {"id":"composite", "type":"Composite"},
            {"id":"screen", "type":"Screen", "params":{"width":16,"height":16,"outputColorSpace":"srgb"}}
        ],
        "connections":edges.into_iter().enumerate().map(|(i,(a,b,c,d))| json!({
            "id":format!("edge.{i}"), "from":{"nodeId":a,"portId":b}, "to":{"nodeId":c,"portId":d}
        })).collect::<Vec<_>>()
    })).unwrap()
}

fn pixels(constant: Option<[f32; 3]>) -> ImageBuffer<Rgba<f32>, Vec<f32>> {
    ImageBuffer::from_fn(8, 4, |x, y| {
        let c = constant.unwrap_or(if y == 0 {
            [16.0, 0.0, 0.0]
        } else if y == 3 {
            [0.0, 0.0, 12.0]
        } else if x < 4 {
            [2.0, 4.0, 1.0]
        } else {
            [8.0, 1.0, 2.0]
        });
        Rgba([c[0], c[1], c[2], 0.0]) // Environment alpha must not erase radiance.
    })
}

fn assets(hdr: bool, constant: Option<[f32; 3]>) -> AssetStore {
    let image = pixels(constant);
    let bytes = if hdr {
        let mut bytes = Vec::new();
        let rgb: Vec<_> = image.pixels().map(|p| Rgb([p[0], p[1], p[2]])).collect();
        image::codecs::hdr::HdrEncoder::new(&mut bytes)
            .encode(&rgb, 8, 4)
            .unwrap();
        bytes
    } else {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba32F(image)
            .write_to(&mut bytes, ImageFormat::OpenExr)
            .unwrap();
        bytes.into_inner()
    };
    let assets = AssetStore::new();
    assets.insert(
        "env",
        AssetData {
            bytes,
            mime_type: if hdr {
                "image/vnd.radiance"
            } else {
                "image/x-exr"
            }
            .into(),
            original_name: if hdr {
                "environment.hdr"
            } else {
                "environment.exr"
            }
            .into(),
        },
    );
    assets
}

fn set(scene: &mut SceneDSL, id: &str, key: &str, value: Value) {
    scene
        .nodes
        .iter_mut()
        .find(|n| n.id == id)
        .unwrap()
        .params
        .insert(key.into(), value);
}

fn render(gpu: &HeadlessRenderer, scene: &SceneDSL, assets: &AssetStore) -> [f32; 4] {
    let built = ShaderSpaceBuilder::new(gpu.device.clone(), gpu.queue.clone())
        .with_adapter(gpu.adapter.clone())
        .with_asset_store(assets.clone())
        .build(scene)
        .unwrap();
    built.shader_space.render();
    let pixels = built
        .shader_space
        .read_texture_rgba16f("target")
        .unwrap()
        .channels;
    assert!(pixels.iter().all(|v| v.is_finite()));
    pixels[(8 * 16 + 8) * 4..(8 * 16 + 8) * 4 + 4]
        .try_into()
        .unwrap()
}

fn near(actual: [f32; 4], expected: [f32; 4]) {
    for (a, b) in actual.into_iter().zip(expected) {
        assert!((a - b).abs() < 0.02, "{actual:?} != {expected:?}");
    }
}

#[test]
fn hdri_hdr_and_exr_preserve_radiance_and_map_rotation_poles_and_seam() {
    let gpu =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    for hdr in [true, false] {
        let assets = assets(hdr, None);
        let mut scene = scene();
        near(render(&gpu, &scene, &assets), [8.0, 1.0, 2.0, 1.0]);
        set(&mut scene, "env", "rotation", json!(180));
        near(render(&gpu, &scene, &assets), [2.0, 4.0, 1.0, 1.0]);
        set(&mut scene, "env", "rotation", json!(90));
        near(render(&gpu, &scene, &assets), [5.0, 2.5, 1.5, 1.0]);
        set(&mut scene, "env", "rotation", json!(89.999));
        let left = render(&gpu, &scene, &assets);
        set(&mut scene, "env", "rotation", json!(90.001));
        near(render(&gpu, &scene, &assets), left);
        set(&mut scene, "env", "rotation", json!(0));
        set(&mut scene, "env", "normal", json!([0, 1, 1]));
        near(render(&gpu, &scene, &assets), [16.0, 0.0, 0.0, 1.0]);
        set(&mut scene, "env", "normal", json!([0, -1, 1]));
        near(render(&gpu, &scene, &assets), [0.0, 0.0, 12.0, 1.0]);
        set(&mut scene, "env", "normalStrength", json!(0));
        near(render(&gpu, &scene, &assets), [8.0, 1.0, 2.0, 1.0]);
        set(&mut scene, "env", "normalStrength", json!(1));
        set(&mut scene, "env", "normal", json!([1, 0, 1]));
        set(
            &mut scene,
            "transform",
            "rotate",
            json!({"x":0,"y":0,"z":90}),
        );
        near(render(&gpu, &scene, &assets), [16.0, 0.0, 0.0, 1.0]);
    }
}

#[test]
fn env_color_and_material_outputs_honor_filter_and_independent_address_modes() {
    let gpu =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    let map = assets(false, None);
    for output in ["material", "color"] {
        let mut input = scene();
        input.connections[0].from.port_id = output.into();
        set(&mut input, "env", "rotation", json!(89.9));
        // Linear sampling crosses the horizontal panorama seam by default.
        near(render(&gpu, &input, &map), [5.0, 2.5, 1.5, 1.0]);
        set(&mut input, "env", "magFilter", json!("nearest"));
        set(&mut input, "env", "minFilter", json!("nearest"));
        near(render(&gpu, &input, &map), [8.0, 1.0, 2.0, 1.0]);
        set(&mut input, "env", "magFilter", json!("linear"));
        set(&mut input, "env", "minFilter", json!("linear"));
        for address in ["clamp-to-edge", "mirror-repeat"] {
            set(&mut input, "env", "addressModeU", json!(address));
            near(render(&gpu, &input, &map), [8.0, 1.0, 2.0, 1.0]);
        }

        set(&mut input, "env", "normal", json!([0, 1, 1]));
        near(render(&gpu, &input, &map), [16.0, 0.0, 0.0, 1.0]);
        set(&mut input, "env", "addressModeV", json!("repeat"));
        near(render(&gpu, &input, &map), [8.0, 0.0, 6.0, 1.0]);
        set(&mut input, "env", "addressModeV", json!("mirror-repeat"));
        near(render(&gpu, &input, &map), [16.0, 0.0, 0.0, 1.0]);
    }
}

#[test]
fn env_roughness_exposure_alpha_and_connected_image_are_independent() {
    let gpu =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    let mut input = scene();
    let constant = assets(false, Some([4.0, 2.0, 8.0]));
    for roughness in [0.0, 0.3, 1.0] {
        set(&mut input, "env", "roughness", json!(roughness));
        near(render(&gpu, &input, &constant), [4.0, 2.0, 8.0, 1.0]);
    }
    set(&mut input, "env", "exposure", json!(1));
    set(&mut input, "env", "intensity", json!(0.5));
    set(&mut input, "env", "tint", json!([1, 0.5, 1, 0.5]));
    set(&mut input, "env", "alpha", json!(0.25));
    near(render(&gpu, &input, &constant), [0.5, 0.125, 1.0, 0.125]);

    let map = assets(true, None);
    input = scene();
    let sharp = render(&gpu, &input, &map);
    set(&mut input, "env", "roughness", json!(1));
    let rough = render(&gpu, &input, &map);
    assert!(
        sharp[..3]
            .iter()
            .zip(&rough[..3])
            .map(|(a, b)| (a - b).abs())
            .sum::<f32>()
            > 0.2
    );
    set(&mut input, "env", "view", json!([0, 0, 0]));
    set(&mut input, "env", "normal", json!([0, 0, 0]));
    render(&gpu, &input, &map);

    input = scene();
    set(&mut input, "env", "assetId", json!("stale-inline-image"));
    input.nodes.push(
        serde_json::from_value(
            json!({"id":"image", "type":"ImageFile", "params":{"assetId":"env"}}),
        )
        .unwrap(),
    );
    input.connections.push(serde_json::from_value(json!({"id":"image", "from":{"nodeId":"image","portId":"image"},"to":{"nodeId":"env","portId":"image"}})).unwrap());
    near(render(&gpu, &input, &map), sharp);

    // The same ImageFile binding also crosses a reusable group boundary.
    input.groups.push(serde_json::from_value(json!({
        "id":"environment-group",
        "inputs":[{"id":"image","name":"HDRI","type":"ImageFile"}],
        "outputs":[{"id":"material","name":"Material","type":"material"}],
        "nodes":[{"id":"material","type":"EnvMaterial","params":{"assetId":"stale-group-image"}}],
        "connections":[],
        "inputBindings":[{"groupPortId":"image","to":{"nodeId":"material","portId":"image"}}],
        "outputBindings":[{"groupPortId":"material","from":{"nodeId":"material","portId":"material"}}]
    })).unwrap());
    let env = input.nodes.iter_mut().find(|n| n.id == "env").unwrap();
    env.node_type = "GroupInstance".into();
    env.params = serde_json::from_value(json!({"groupId":"environment-group"})).unwrap();
    near(render(&gpu, &input, &map), sharp);
}
