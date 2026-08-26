use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use node_forge_render_server::{
    asset_store::{AssetData, AssetStore},
    dsl::SceneDSL,
    playback::PlaybackRuntime,
    protocol::{
        InteractionEventData, InteractionEventPayload, InteractionKeyData, InteractionPosition,
        InteractionWheelData,
    },
    renderer::node_compiler::template_loader,
    state_machine::graph_function::{self, FunctionResource},
};
use rust_wgpu_fiber::eframe::{self, egui, wgpu};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use web_sys::HtmlCanvasElement;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaybackBundle {
    scene: SceneDSL,
    #[serde(default)]
    assets: HashMap<String, ArchiveAsset>,
    #[serde(default)]
    materials: HashMap<String, ArchiveMaterial>,
    #[serde(default)]
    functions: HashMap<String, FunctionResource>,
}

#[derive(Deserialize)]
struct ArchiveAsset {
    metadata: ArchiveAssetMetadata,
    #[serde(with = "serde_bytes")]
    content: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveAssetMetadata {
    original_name: String,
    mime_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveMaterial {
    node_id: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchEvent {
    #[serde(rename = "type")]
    event_type: String,
    position: Option<DispatchPosition>,
    button: Option<i16>,
    delta: Option<DispatchPosition>,
    key: Option<String>,
    code: Option<String>,
    #[serde(default)]
    repeat: bool,
}

#[derive(Deserialize)]
struct DispatchPosition {
    x: f32,
    y: f32,
}

struct PlayerApp {
    runtime: PlaybackRuntime,
    texture_id: egui::TextureId,
    context: egui::Context,
    diagnostic: js_sys::Function,
    device_lost: Arc<Mutex<Option<String>>>,
    viewport: [u32; 4],
    sequence: u64,
}

impl PlayerApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        bundle: PlaybackBundle,
        diagnostic: js_sys::Function,
    ) -> anyhow::Result<Self> {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WebGPU render state is unavailable"))?;
        let device_lost = Arc::new(Mutex::new(None));
        let device_lost_callback = Arc::clone(&device_lost);
        render_state
            .device
            .set_device_lost_callback(move |reason, message| {
                if reason != wgpu::DeviceLostReason::Destroyed {
                    *device_lost_callback.lock().unwrap() = Some(format!(
                        "WebGPU device lost ({reason:?}){}",
                        if message.is_empty() {
                            String::new()
                        } else {
                            format!(": {message}")
                        }
                    ));
                }
            });
        template_loader::install_document_overrides(
            bundle
                .materials
                .into_values()
                .map(|material| (material.node_id, material.content)),
        );
        graph_function::validate_function_resources(
            &bundle.scene,
            &bundle.functions.values().cloned().collect::<Vec<_>>(),
        )?;
        graph_function::install_document_functions(bundle.functions.into_values())?;

        let assets = AssetStore::new();
        for (id, asset) in bundle.assets {
            assets.insert(
                id,
                AssetData {
                    bytes: asset.content,
                    mime_type: asset.metadata.mime_type,
                    original_name: asset.metadata.original_name,
                },
            );
        }
        let mut runtime = PlaybackRuntime::new(
            Arc::new(render_state.device.clone()),
            Arc::new(render_state.queue.clone()),
            render_state.adapter.clone(),
            bundle.scene,
            assets,
        )?;
        runtime.advance(0.0)?;
        let resolution = runtime.resolution();
        let texture_id = render_state.renderer.write().register_native_texture(
            &render_state.device,
            runtime
                .present_texture_view()
                .ok_or_else(|| anyhow::anyhow!("playback present texture is unavailable"))?,
            wgpu::FilterMode::Linear,
        );
        Ok(Self {
            runtime,
            texture_id,
            context: cc.egui_ctx.clone(),
            diagnostic,
            device_lost,
            viewport: [0, 0, resolution[0], resolution[1]],
            sequence: 0,
        })
    }

    fn advance(&mut self, delta_seconds: f64) -> Result<bool, JsValue> {
        if let Some(message) = self.device_lost.lock().unwrap().clone() {
            return Err(JsValue::from_str(&message));
        }
        let frame = self.runtime.advance(delta_seconds).map_err(to_js_error)?;
        for message in frame.diagnostics {
            let value = serde_json::json!({
                "level": "warning",
                "code": "runtime",
                "message": message,
            })
            .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
            .map_err(|error| to_js_error(anyhow::anyhow!(error)))?;
            let _ = self.diagnostic.call1(&JsValue::NULL, &value);
        }
        self.context.request_repaint();
        Ok(frame.ended)
    }

    fn dispatch(&mut self, event: DispatchEvent) -> Result<(), JsValue> {
        self.sequence = self.sequence.saturating_add(1);
        let position = event.position.map(|position| InteractionPosition {
            client_x: position.x,
            client_y: position.y,
            canvas_x: position.x,
            canvas_y: position.y,
            frag_pixel_x: Some(position.x),
            frag_pixel_y: Some(position.y),
        });
        let wheel = event.delta.map(|delta| InteractionWheelData {
            delta_x: delta.x,
            delta_y: delta.y,
            delta_mode: 0,
        });
        let key = event.key.map(|key| InteractionKeyData {
            key,
            physical_key: event.code,
            repeat: event.repeat,
        });
        self.runtime.dispatch(InteractionEventPayload {
            event_type: event.event_type,
            seq: self.sequence,
            data: Some(InteractionEventData {
                position,
                key,
                button: event.button.map(button_name),
                wheel,
                ..Default::default()
            }),
        });
        self.advance(0.0).map(|_| ())
    }

    fn set_viewport(&mut self, viewport: [u32; 4]) -> Result<(), JsValue> {
        let resolution = self.runtime.resolution();
        validate_viewport(viewport, resolution).map_err(|message| JsValue::from_str(&message))?;
        self.viewport = viewport;
        self.context.request_repaint();
        Ok(())
    }
}

impl eframe::App for PlayerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let available = ui.max_rect();
        ui.painter()
            .rect_filled(available, 0.0, egui::Color32::BLACK);
        let resolution = self.runtime.resolution();
        ui.painter().image(
            self.texture_id,
            available,
            viewport_uv_rect(self.viewport, resolution),
            egui::Color32::WHITE,
        );
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn validate_viewport(viewport: [u32; 4], resolution: [u32; 2]) -> Result<(), String> {
    let [x, y, width, height] = viewport;
    let right = x.checked_add(width);
    let top = y.checked_add(height);
    if width == 0
        || height == 0
        || right.is_none_or(|right| right > resolution[0])
        || top.is_none_or(|top| top > resolution[1])
    {
        return Err(format!(
            "viewport [{x}, {y}, {width}, {height}] exceeds scene bounds {}x{}",
            resolution[0], resolution[1]
        ));
    }
    Ok(())
}

fn viewport_uv_rect(viewport: [u32; 4], resolution: [u32; 2]) -> egui::Rect {
    let [x, y, width, height] = viewport;
    let scene_width = resolution[0].max(1) as f32;
    let scene_height = resolution[1].max(1) as f32;
    egui::Rect::from_min_max(
        egui::pos2(
            x as f32 / scene_width,
            1.0 - (y + height) as f32 / scene_height,
        ),
        egui::pos2(
            (x + width) as f32 / scene_width,
            1.0 - y as f32 / scene_height,
        ),
    )
}

#[wasm_bindgen]
pub struct WebPlayerBackend {
    canvas_id: String,
    runner: eframe::WebRunner,
    diagnostic: js_sys::Function,
}

#[wasm_bindgen]
impl WebPlayerBackend {
    #[wasm_bindgen(constructor)]
    pub fn new(canvas_id: String, diagnostic: js_sys::Function) -> Self {
        eframe::WebLogger::init(log::LevelFilter::Warn).ok();
        Self {
            canvas_id,
            runner: eframe::WebRunner::new(),
            diagnostic,
        }
    }

    pub async fn load(&self, bundle: JsValue) -> Result<(), JsValue> {
        let bundle: PlaybackBundle = serde_wasm_bindgen::from_value(bundle)
            .map_err(|error| to_js_error(anyhow::anyhow!(error)))?;
        let canvas = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(&self.canvas_id))
            .ok_or_else(|| JsValue::from_str("player canvas not found"))?
            .dyn_into::<HtmlCanvasElement>()?;
        let mut options = eframe::WebOptions::default();
        options.renderer = eframe::Renderer::Wgpu;
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup
        {
            setup.instance_descriptor.backends = wgpu::Backends::BROWSER_WEBGPU;
        }
        let diagnostic = self.diagnostic.clone();
        self.runner
            .start(
                canvas,
                options,
                Box::new(move |cc| Ok(Box::new(PlayerApp::new(cc, bundle, diagnostic.clone())?))),
            )
            .await
    }

    pub fn play(&self) {
        if let Some(mut app) = self.runner.app_mut::<PlayerApp>() {
            app.runtime.play();
        }
    }

    pub fn pause(&self) {
        if let Some(mut app) = self.runner.app_mut::<PlayerApp>() {
            app.runtime.pause();
        }
    }

    pub fn reset(&self) -> Result<(), JsValue> {
        if let Some(mut app) = self.runner.app_mut::<PlayerApp>() {
            app.runtime.reset().map_err(to_js_error)?;
            app.context.request_repaint();
        }
        Ok(())
    }

    pub fn advance(&self, delta_seconds: f64) -> Result<bool, JsValue> {
        self.runner
            .app_mut::<PlayerApp>()
            .map(|mut app| app.advance(delta_seconds))
            .unwrap_or_else(|| Err(JsValue::from_str("player runtime is unavailable")))
    }

    pub fn dispatch(&self, event: JsValue) -> Result<(), JsValue> {
        let event = serde_wasm_bindgen::from_value(event)
            .map_err(|error| to_js_error(anyhow::anyhow!(error)))?;
        if let Some(mut app) = self.runner.app_mut::<PlayerApp>() {
            app.dispatch(event)?;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = setViewport)]
    pub fn set_viewport(&self, viewport: JsValue) -> Result<(), JsValue> {
        let viewport = serde_wasm_bindgen::from_value(viewport)
            .map_err(|error| to_js_error(anyhow::anyhow!(error)))?;
        self.runner
            .app_mut::<PlayerApp>()
            .map(|mut app| app.set_viewport(viewport))
            .unwrap_or_else(|| Err(JsValue::from_str("player runtime is unavailable")))
    }

    #[wasm_bindgen(js_name = sceneSize)]
    pub fn scene_size(&self) -> Result<JsValue, JsValue> {
        let resolution = self
            .runner
            .app_mut::<PlayerApp>()
            .map(|app| app.runtime.resolution())
            .ok_or_else(|| JsValue::from_str("player runtime is unavailable"))?;
        serde_json::json!({
            "width": resolution[0],
            "height": resolution[1],
        })
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|error| to_js_error(anyhow::anyhow!(error)))
    }

    #[wasm_bindgen(js_name = currentStateId)]
    pub fn current_state_id(&self) -> Option<String> {
        self.runner
            .app_mut::<PlayerApp>()
            .and_then(|app| app.runtime.current_state_id().map(str::to_owned))
    }

    pub fn resize(&self, _width: f64, _height: f64, _device_pixel_ratio: f64) {}

    pub fn destroy(&self) {
        self.runner.destroy();
        graph_function::clear_document_functions();
        template_loader::install_document_overrides(std::iter::empty());
    }
}

fn button_name(button: i16) -> String {
    match button {
        0 => "left",
        1 => "middle",
        2 => "right",
        _ => "unknown",
    }
    .to_string()
}

fn to_js_error(error: anyhow::Error) -> JsValue {
    JsValue::from_str(&format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::{validate_viewport, viewport_uv_rect};

    #[test]
    fn viewport_is_bounded_by_authored_scene() {
        assert!(validate_viewport([0, 2080, 1260, 720], [1260, 2800]).is_ok());
        assert!(validate_viewport([0, 0, 1260, 2801], [1260, 2800]).is_err());
        assert!(validate_viewport([0, 0, 0, 100], [1260, 2800]).is_err());
    }

    #[test]
    fn viewport_scene_y_up_maps_to_raster_uv() {
        let uv = viewport_uv_rect([20, 100, 80, 200], [200, 400]);
        assert!((uv.min.x - 0.1).abs() < f32::EPSILON);
        assert!((uv.max.x - 0.5).abs() < f32::EPSILON);
        assert!((uv.min.y - 0.25).abs() < f32::EPSILON);
        assert!((uv.max.y - 0.75).abs() < f32::EPSILON);
    }
}
