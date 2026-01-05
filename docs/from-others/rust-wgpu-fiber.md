# rust-wgpu-fiber（按本项目初始化构建 pipeline 的用法）

> 本文档以 [src/main.rs](../src/main.rs) 为准（你已将 pipeline/资源构建放到初始化，而不是 `update()`）。
> 核心入口是 `shader_space::ShaderSpace`，你可以把它理解成一个轻量的“帧图/渲染编排层”：**先声明资源与 pass，再在每帧执行 render**。

---

## 1. 依赖与运行环境

本项目的依赖写法：

```toml
[dependencies]
rust-wgpu-fiber = { version = "*", path = "../../rust-wgpu-fiber" }
```

本项目通过 `rust_wgpu_fiber::eframe` 复用：

- `eframe/egui`：窗口与 UI
- `wgpu`：图形后端（从 `rust_wgpu_fiber::eframe::wgpu` 引入）

---

## 2. 关键变化：pipeline 在初始化构建

你现在的结构是：

- **初始化阶段（只做一次）**：
  - 创建 `ShaderSpace`
  - 声明 buffers / textures
  - 定义多个 `render_pass`
  - 用 `composite` 固定 pass 执行顺序
  - 调用 `prepare()` 完成 pipeline/绑定资源准备
- **每帧 update**：
  - 只调用 `shader_space.render()` 提交渲染
  - 首帧把渲染结果纹理注册为 egui 纹理（`TextureId`），之后复用

这个模型适合“渲染管线不变、每帧只是执行”的场景；如果你需要动态改分辨率/改 shader/改 pass 顺序，则需要在变化发生时重新走一遍“声明 + prepare”。

---

## 3. 快速开始（与你现在的 main.rs 同结构）

下面的代码骨架与 [src/main.rs](../src/main.rs) 一致：

- `ShaderSpace` 在 `run_native` 的初始化闭包里搭建
- `update()` 里只 `render()` + 显示纹理

```rust
use std::sync::Arc;

use rust_wgpu_fiber::{
    eframe::{
        self,
        egui::{self, Color32, Rect, TextureId, pos2},
        wgpu::{self, BlendState, Color, TextureUsages, include_wgsl, vertex_attr_array},
    },
    env_logger,
    shader_space::ShaderSpace,
};

struct App<'a> {
    shader_space: ShaderSpace<'a>,
    resolution: [u32; 2],
    color_attachment: Option<TextureId>,
}

impl<'a> eframe::App for App<'a> {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let render_state = frame.wgpu_render_state().unwrap();

        // 每帧只执行渲染（pipeline 已在初始化 prepare 过）
        self.shader_space.render();

        // 首次把输出纹理注册给 egui
        if self.color_attachment.is_none() {
            let mut renderer = frame.wgpu_render_state().unwrap().renderer.as_ref().write();
            let texture = self.shader_space.textures.get("render_attachment").unwrap();

            self.color_attachment = Some(renderer.register_native_texture(
                &render_state.device,
                texture.wgpu_texture_view.as_ref().unwrap(),
                eframe::wgpu::FilterMode::Linear,
            ));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.centered_and_justified(|ui| {
                ui.painter().image(
                    self.color_attachment.unwrap(),
                    Rect::from_min_max(
                        pos2(0.0, 0.0),
                        pos2(self.resolution[0] as f32, self.resolution[1] as f32),
                    ),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            });
        });
    }
}

fn main() -> eframe::Result {
    env_logger::init();

    let resolution = [600, 400];

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_resizable(false)
            .with_transparent(true)
            .with_inner_size(resolution.map(|x| x as f32))
            .with_min_inner_size(resolution.map(|x| x as f32)),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    eframe::run_native("Alpha Composition Tests", native_options, Box::new(|cc| {
        let render_state = cc.wgpu_render_state.as_ref().unwrap();

        // 1) 初始化 ShaderSpace
        let mut shader_space = ShaderSpace::new(
            Arc::new(render_state.device.clone()),
            Arc::new(render_state.queue.clone()),
        );

        // 2) 声明资源 + pass + composite（只做一次）
        shader_space
            .buffer_pool(|builder| builder.plane_geometry("plane_geometry"))
            .texture_pool(|builder| {
                builder.texture(
                    "render_attachment",
                    resolution,
                    wgpu::TextureFormat::Rgba8Unorm,
                    TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_SRC,
                )
            })
            .render_pass("premul_RED", |builder| {
                builder
                    .shader(include_wgsl!("./shaders/premul_RED.wgsl"))
                    .bind_attribute_buffer(
                        0,
                        "plane_geometry",
                        eframe::wgpu::VertexStepMode::Vertex,
                        vertex_attr_array![0 => Float32x3].to_vec(),
                    )
                    .bind_color_attachment("render_attachment")
                    .blending(BlendState::REPLACE)
                    .load_op(wgpu::LoadOp::Clear(Color::TRANSPARENT))
            })
            // ... 其它 render_pass 省略
            .composite(|composer| composer.pass("premul_RED"));

        // 3) 初始化完成后 prepare 一次
        shader_space.prepare();

        Ok(Box::new(App {
            shader_space,
            resolution,
            color_attachment: None,
        }))
    }))
}
```

---

## 4. API 心智模型（按你现在用到的部分）

### 4.1 `ShaderSpace::new(device, queue)`

- 持有 `wgpu::Device`/`wgpu::Queue`（你这里用 `Arc` 包起来，方便共享/克隆）。

### 4.2 `buffer_pool` / `texture_pool`

- 用 name 注册/复用资源。
- 你的例子：
  - `.plane_geometry("plane_geometry")`
  - `.texture("render_attachment", resolution, format, usages)`

### 4.3 `render_pass(name, |builder| ...)`

你在 pass builder 里用到：

- `.shader(include_wgsl!(...))`
- `.bind_attribute_buffer(slot, buffer_name, step_mode, attrs)`
- `.bind_color_attachment(texture_name)`
- `.blending(BlendState::...)`
- `.load_op(LoadOp::{Load, Clear(...)})`

### 4.4 `composite(|composer| ...)`

- 用 `.pass("...")` 指定执行顺序。
- 你当前用它做了 premul 与 straight 两组对比链。

---

## 5. 生命周期与调用顺序（更新版）

推荐顺序（与你现在一致）：

1) 初始化闭包中：声明 buffers/textures/passes/composite
2) 调用一次 `shader_space.prepare()`
3) 每帧 `update()`：只调用 `shader_space.render()`
4) 首帧把 `render_attachment` 注册给 egui，后续只复用 `TextureId`

什么时候需要“重新 prepare”？

- 分辨率变化（需要重建 `render_attachment`）
- shader/pipeline 配置变化（blend、vertex layout、attachment format 等）
- pass 列表或 composite 顺序变化

实践建议：

- 把“可变参数”（例如分辨率）集中管理；一旦变化，就重新走一遍：`texture_pool`/`render_pass`/`composite` + `prepare()`。

---

## 6. 与 egui/eframe 集成要点

- 输出纹理必须有 `TextureUsages::TEXTURE_BINDING`，否则无法给 egui 采样。
- `TextureId` 只注册一次：你用 `Option<TextureId>` 缓存是正确的。
- 注册所需的 `TextureView` 来自：`shader_space.textures.get("render_attachment")`。

---

## 7. 常见问题（按“初始化构建”模式）

### 7.1 画面不更新

- 确认 `update()` 每帧都调用了 `shader_space.render()`。
- 确认初始化阶段调用了 `shader_space.prepare()`。
- 确认 `composite()` 里包含了你想跑的 pass。

### 7.2 TextureId 注册后仍是黑屏

- `render_attachment` 是否真的被 pass 写入（检查 `.bind_color_attachment("render_attachment")`）。
- 第一个 pass 的 `load_op` 是否正确（例如你需要清屏却用了 `Load`）。

---

## 8. 术语对照（如果你内部叫 rust-three-fiber）

- `ShaderSpace`：渲染根/编排器
- `buffer_pool` / `texture_pool`：资源声明（按 name 管理与复用）
- `render_pass`：一个命名的渲染步骤（pipeline + bind + attachment）
- `composite`：本帧（或本管线）的 pass 执行序列
