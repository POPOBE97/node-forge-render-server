use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    renderer::ShaderSpaceBuilder,
};
use rust_wgpu_fiber::{HeadlessRenderer, HeadlessRendererConfig};
use serde_json::json;

fn scene() -> SceneDSL {
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
    serde_json::from_value(json!({"version":"6.0", "workingColorSpace":"linear-srgb", "metadata":{"name":"text-sdf-pbr"},
        "nodes":[
            {"id":"font","type":"FontFile","params":{"assetId":"latin"}},
            {"id":"text","type":"TextSdf","params":{"text":"O5", "fontSize":90, "size":[200,140], "align":"center", "verticalAlign":"center"}},
            {"id":"bevel","type":"Sdf2DBevel","params":{"width":3,"cliff":0.5,"sampleDistance":0.35}},
            {"id":"pbr","type":"PBRMaterial","params":{"baseColor":[0.04,0.4,0.3,0.6], "metallic":0.3, "roughness":0.32, "normalStrength":-0.8,"lightDirection":[-0.8,0.5,1],"lightIntensity":3}},
            {"id":"geometry","type":"Rect2DGeometry","params":{"size":[200,140],"position":[0,0]}},
            {"id":"transform","type":"TransformGeometry","params":{"translate":{"x":128,"y":128,"z":0},"rotate":{"x":0,"y":0,"z":0}}},
            {"id":"draw","type":"RenderPass","params":{"blend_preset":"premul_alpha"}},
            {"id":"target","type":"RenderTexture","params":{"width":256,"height":256,"format":"rgba16float"}},
            {"id":"composite","type":"Composite","params":{}},
            {"id":"screen","type":"Screen","params":{"outputColorSpace":"srgb","width":256,"height":256}}
        ],"connections":edges.iter().enumerate().map(|(i,(a,b,c,d))| json!({"id":format!("edge.{i}"),"from":{"nodeId":a,"portId":b},"to":{"nodeId":c,"portId":d}})).collect::<Vec<_>>(),"outputs":null
    })).unwrap()
}

#[test]
fn text_sdf_glass_transmits_background_and_refracts_without_filling_counters() {
    let renderer = HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    let assets = AssetStore::new();
    assets.insert("latin", AssetData {
        bytes: include_bytes!("../assets/fonts/GeistMono/GeistMono-Regular.ttf").to_vec(),
        mime_type: "font/ttf".into(), original_name: "GeistMono-Regular.ttf".into(),
    });
    let image = image::RgbaImage::from_fn(256, 256, |x, y| {
        image::Rgba([if (x / 9 + y / 13) % 2 == 0 { 220 } else { 30 }, (y % 256) as u8, 100, 255])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    assets.insert("pattern", AssetData { bytes: bytes.into_inner(), mime_type: "image/png".into(), original_name: "pattern.png".into() });
    let mut scene = scene();
    let glass = scene.nodes.iter_mut().find(|n| n.id == "pbr").unwrap();
    glass.node_type = "GlassMaterial".into();
    glass.params = serde_json::from_value(json!({
        "uAlpha":0.55, "uNormalStrength":-0.7, "uShapeEdgePx":3, "uShapeThicknessPx":8,
        "uShapeReflectOffsetPx":0, "uReflectStrength":1, "uRefractIorIOR":1.46,
        "uDirectionalLightIntensity":0, "uDirectionalLightOppositeIntensity":0,
        "uInnerGlassColor":[1,1,1,0], "uInnerColorMix":0, "uInnerColorPow":1,
        "uInnerColorWhite":0, "uInnerBottom":0, "uBgColorSaturation":1,
        "uDebugFixNeutralVibrancy":0, "uBionicUnShade":0
    })).unwrap();
    scene.connections.retain(|e| !(e.to.node_id == "pbr" && e.to.port_id == "alpha"));
    for edge in &mut scene.connections {
        if edge.to.node_id == "pbr" && edge.to.port_id == "normal" { edge.to.port_id = "uNormal".into(); }
    }
    for value in [
        json!({"id":"bg.image","type":"ImageTexture","params":{"assetId":"pattern"}}),
        json!({"id":"bg.geometry","type":"Rect2DGeometry","params":{"size":[256,256],"position":[128,128]}}),
        json!({"id":"bg.draw","type":"RenderPass","params":{}}),
    ] { scene.nodes.push(serde_json::from_value(value).unwrap()); }
    for (i, (a,b,c,d)) in [
        ("text","distance","pbr","uSdfDistance"),
        ("bg.image","color","bg.draw","material"),
        ("bg.geometry","geometry","bg.draw","geometry"),
        ("bg.draw","texture","pbr","uBgTex"),
    ].into_iter().enumerate() {
        scene.connections.push(serde_json::from_value(json!({"id":format!("glass.{i}"),"from":{"nodeId":a,"portId":b},"to":{"nodeId":c,"portId":d}})).unwrap());
    }
    let render = |scene: &SceneDSL| {
        let output = ShaderSpaceBuilder::new(renderer.device.clone(), renderer.queue.clone())
            .with_adapter(renderer.adapter.clone()).with_asset_store(assets.clone()).build(scene).unwrap();
        output.shader_space.render();
        output.shader_space.read_texture_rgba16f("target").unwrap().channels
    };
    let original = render(&scene);
    assert!(original.iter().all(|v| v.is_finite()));
    assert!(original.chunks_exact(4).filter(|p| p[3] > 0.5).count() > 1000);
    assert!(original.chunks_exact(4).all(|p| p[3] <= 0.551));
    let change = |scene: &mut SceneDSL, port: &str, value| {
        scene.nodes.iter_mut().find(|n| n.id == "pbr").unwrap().params.insert(port.into(), value);
    };
    change(&mut scene, "uRefractIorIOR", json!(1.0));
    let unrefracted = render(&scene);
    let diff: f32 = original.iter().zip(&unrefracted).map(|(a,b)| (a-b).abs()).sum();
    assert!(diff > 10.0, "external glyph normals must refract the patterned background: {diff}");
    change(&mut scene, "uDirectionalLightIntensity", json!(0.6));
    let lit = render(&scene);
    assert!(lit.chunks_exact(4).zip(unrefracted.chunks_exact(4)).all(|(a,b)| (a[3]-b[3]).abs() < 0.001), "lighting must not change transparency or fill glyph counters");
    scene.nodes.iter_mut().find(|n| n.id == "text").unwrap().params.insert("text".into(), json!(""));
    assert!(render(&scene).chunks_exact(4).all(|p| p[3] == 0.0));
}

#[test]
fn text_sdf_bevel_pbr_responds_to_light_roughness_and_geometry_rotation() {
    let renderer =
        HeadlessRenderer::new(HeadlessRendererConfig::default()).expect("native GPU required");
    let assets = AssetStore::new();
    assets.insert(
        "latin",
        AssetData {
            bytes: include_bytes!("../assets/fonts/GeistMono/GeistMono-Regular.ttf").to_vec(),
            mime_type: "font/ttf".into(),
            original_name: "GeistMono-Regular.ttf".into(),
        },
    );
    let render = |scene: &SceneDSL| {
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
    };
    let mut scene = scene();
    let original = render(&scene);
    assert!(original.iter().all(|v| v.is_finite()));
    let pixels: Vec<_> = original.chunks_exact(4).collect();
    assert!(pixels.iter().filter(|p| p[3] > 0.5).count() > 1000);
    assert!(pixels.iter().all(|p| p[3] <= 0.601));
    let set = |scene: &mut SceneDSL, id: &str, key: &str, value| {
        scene
            .nodes
            .iter_mut()
            .find(|n| n.id == id)
            .unwrap()
            .params
            .insert(key.into(), value);
    };
    set(&mut scene, "pbr", "lightDirection", json!([0.8, -0.5, 1]));
    let relit = render(&scene);
    let difference =
        |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum::<f32>();
    assert!(
        difference(&original, &relit) > 20.0,
        "SDF normals must respond to lighting"
    );
    assert!(
        original
            .chunks_exact(4)
            .zip(relit.chunks_exact(4))
            .all(|(a, b)| (a[3] - b[3]).abs() < 0.001)
    );
    set(&mut scene, "pbr", "roughness", json!(0.85));
    assert!(
        difference(&relit, &render(&scene)) > 5.0,
        "GGX roughness must alter highlights"
    );
    // Rotate geometry and light together. The material must follow the local SDF frame.
    scene = self::scene();
    set(
        &mut scene,
        "transform",
        "rotate",
        json!({"x":0,"y":0,"z":90}),
    );
    set(&mut scene, "pbr", "lightDirection", json!([-0.5, -0.8, 1]));
    let rotated = render(&scene);
    let mut error = 0.0;
    for y in 0..256 {
        for x in 0..256 {
            for c in 0..4 {
                error += (original[(y * 256 + x) * 4 + c] - rotated[((255 - x) * 256 + y) * 4 + c])
                    .abs();
            }
        }
    }
    assert!(error < 20.0, "rotation/light frame mismatch: {error}");
    set(&mut scene, "text", "text", json!(""));
    assert!(
        render(&scene).chunks_exact(4).all(|p| p[3] == 0.0),
        "empty text must be transparent"
    );
}
