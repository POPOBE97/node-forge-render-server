# 03 MeshGradient Canvas Design Session Plan

## Context

MeshGradient design 入口保持在 render server 的 pass dependency tree：用户右键对应的 pass node，然后选择 `Design`。不增加 editor 侧触发，也不新增 editor -> render server 的 design request 协议。

当前实现把 `Design` 打开到独立 pass design window。目标是删除这个 window 模型，把 MeshGradient 的控制点和颜色编辑直接合并到 render server 主 canvas 中。

## Decisions

- 不在进入 design session 时 reset viewport。当前 zoom/pan 原样保留，用户需要重置视图时手动按现有 `R`。
- 不保留 window 兼容层。迁移完成后移除 `PassDesignWindowMap`、window open/show/sync 相关代码和 `ui::pass_design_window` 模块。
- editor 侧不改入口；只继续接收现有 `design_param_patch` 回写。
- 同一时间只允许一个 canvas design session。新的 `Design` 会替换当前 design target，但保留进入第一段 design 前的 preview 状态用于退出恢复。
- design session 是 canvas 编辑模式，不是普通 preview。它会临时接管部分 canvas 交互。

## Target State

Design mode needs a generic session shell plus pass-specific tool state. Do not add MeshGradient fields directly to the top-level canvas/session state.

Define shared design-mode state in `src/app/canvas/design/state.rs`, then store it from the existing `CanvasState` in `src/app/canvas/state.rs`:

```rust
pub struct CanvasDesignState {
    pub active: Option<CanvasDesignSession>,
}

pub struct CanvasDesignSession {
    pub target: PassDesignTarget,
    pub session_id: String,
    pub previous_preview_texture: Option<ResourceName>,
    pub owns_preview_texture: bool,
    pub tool: CanvasDesignToolState,
}

pub enum CanvasDesignToolState {
    MeshGradient(MeshGradientDesignState),
    // Future variants live here:
    // ColorGrading(ColorGradingDesignState),
    // BlurMask(BlurMaskDesignState),
    // CompositePlacement(CompositePlacementDesignState),
}

pub struct MeshGradientDesignState {
    pub selected_point: usize,
    pub active_drag_point: Option<usize>,
    pub color_popover_point: Option<usize>,
    pub color_popover_state: ColorPopoverState,
    pub optimistic_params: HashMap<String, Value>,
}
```

`CanvasState` 增加：

```rust
pub design: CanvasDesignState
```

This keeps common session mechanics separate from controller-specific state:

- Common session: target identity, session id, preview restore, active tool variant.
- Tool state: selected handles, active drags, temporary params, popovers, per-pass interaction state.
- No `Option<mesh_gradient_...>` fields on the generic session.

Design logic should live under a small module tree:

```text
src/app/canvas/design/
  mod.rs          // public integration boundary used by presenter/reducer
  state.rs        // CanvasDesignState, CanvasDesignSession, CanvasDesignToolState
  registry.rs     // target -> supported design tool
  interaction.rs  // shared claim/priority types
  mesh_gradient.rs
```

The registry maps `PassDesignTarget` to a tool kind:

```rust
pub enum CanvasDesignToolKind {
    MeshGradient,
}

pub fn tool_kind_for_target(target: &PassDesignTarget) -> Option<CanvasDesignToolKind> {
    match target.node_type.as_str() {
        "MeshGradient" => Some(CanvasDesignToolKind::MeshGradient),
        _ => None,
    }
}
```

Tool behavior uses enum dispatch from `CanvasDesignToolState`, not one giant polymorphic state struct. The top-level presenter/reducer calls shared entry points such as:

```rust
pub fn enter_session(
    current: Option<CanvasDesignSession>,
    target: PassDesignTarget,
    previous_preview_texture: Option<ResourceName>,
) -> Option<CanvasDesignSession>;

pub fn show_active_overlay(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    session: &mut CanvasDesignSession,
    input: DesignOverlayInput<'_>,
) -> DesignOverlayOutput;
```

`show_active_overlay` dispatches to `mesh_gradient::show_overlay(...)` for the MeshGradient variant. Future pass tools add a new state variant and one module without changing the common session fields.

`DesignOverlayInput` should be controller-neutral:

```rust
pub struct DesignOverlayInput<'a> {
    pub scene: Option<&'a SceneDSL>,
    pub resource_snapshot: Option<&'a ResourceSnapshot>,
    pub editor_connected: bool,
    pub canvas_rect: egui::Rect,
    pub image_rect: egui::Rect,
    pub display_resolution: [u32; 2],
    pub pointer_response: &'a egui::Response,
}

pub struct DesignOverlayOutput {
    pub patches: Vec<DesignParamPatchPayload>,
    pub claims: DesignInteractionClaims,
    pub status: DesignOverlayStatus,
}

pub struct DesignInteractionClaims {
    pub primary_pointer: bool,
    pub suppress_pixel_sampling: bool,
    pub suppress_reference_drag: bool,
    pub suppress_analysis_overlays: bool,
}
```

MeshGradient controller responsibilities:

- 从 `SceneDSL` + `PassDesignTarget` 读取 MeshGradient values。
- 检查 pass/node 是否仍有效。
- 计算 locked ports。
- 在 `image_rect` 上绘制 handles。
- 处理 handle hit-test、drag、color popover。
- 生成 `DesignParamPatchPayload`。

## Entry Flow

入口仍然是：

1. `src/ui/file_tree_widget.rs`
   - pass row context menu 保持 `Design`。
   - 只在 `source_node_type == Some("MeshGradient")` 时提供。
   - 继续返回 `PassDesignTarget`。

2. `src/ui/debug_sidebar.rs`
   - `SidebarAction::OpenPassDesign(PassDesignTarget)` 保持。

3. `src/app/frame/commands.rs`
   - `AppCommand::OpenPassDesign(target)` 不再打开 window。
   - 改为 dispatch canvas action，例如 `CanvasAction::EnterPassDesign(target)`。

4. `src/app/canvas/reducer.rs`
   - `EnterPassDesign` 创建/替换 `app.canvas.design.active`。
   - 如果 `target.target_texture` 存在，设置 `app.canvas.display.preview_texture_name` 为该 texture。
   - 不设置 `pending_view_reset`。
   - 标记 preview/display invalidation，使下一帧同步 texture。
   - 使用 registry 创建对应 `CanvasDesignToolState`。不在 reducer 中写 MeshGradient 交互细节。

退出 flow：

- `CanvasAction::ExitPassDesign` 清空 `app.canvas.design.active`。
- 如果 session 拥有 preview texture，则恢复 `previous_preview_texture`。
- 不 reset viewport。

## Window Removal

迁移不是兼容重定向，直接删除 window 相关结构：

- 删除 `src/ui/pass_design_window.rs`。
- 删除 `src/ui/mod.rs` 中的 `pub mod pass_design_window;`。
- 删除 `AppShell.pass_design_windows`。
- 删除 `App::new` 里的 `PassDesignWindowMap::default()`。
- 删除 `src/app/frame/present.rs` 中：
  - `sync_pass_design_window_textures(...)`
  - `show_pass_design_windows(...)`
- 删除 `src/app/frame/commands.rs` 中对 `open_pass_design_window(...)` 的调用。

保留的协议：

- `DesignParamPatchPayload`
- `AppCommand::SendDesignParamPatch`
- `ws::broadcast_design_param_patch`

## Canvas Rendering Integration

在 `src/app/canvas/presenter.rs` 的 single-scene 分支中集成：

1. 正常构建 display frame。
2. 如果 design session active，强制当前 display source 是 `target.target_texture` 对应 preview。
3. 绘制 base target texture。
4. 在同一个 `image_rect` 上绘制 MeshGradient handles。
5. 在 handles 之上显示 color popover。
6. 再绘制 design badge / stale badge。

坐标映射使用当前 canvas 的 `image_rect`，而不是旧 window 的 `fit_aspect_rect`。也就是说：

- MeshGradient 参数仍是 target pixel space。
- `posN -> screen` 基于 `image_rect`。
- `screen -> posN` 也基于 `image_rect`。
- 可见区域由 `canvas_rect` clip，handle 可以随 zoom/pan 出入视口。

## Interaction Arbitration

Design mode 必须显式和现有 canvas 交互仲裁。

### Keyboard

- `Esc`
  - 如果 color popover 打开：先关闭 popover。
  - 否则退出 design session，并恢复进入 design 前的 preview texture。
  - 不触发现有 `ClearPreviewTexture`。
- `R`
  - 继续执行 reset viewport。进入 design 不自动 reset，但用户手动 reset 仍可用。
- `N` / `S` / `W` / `Space`
  - 保持现有 viewport sampling、HDR clamp、wireframe、pause 语义。
- `F`
  - 保持 toggle canvas-only。
- shortcut gating 仍尊重 `ctx.egui_wants_keyboard_input()`；color popover 编辑输入时不要触发 canvas shortcut。

### Pointer Primary Button

Design session active 时，primary button 优先级：

1. 如果按下位置命中可编辑 handle：
   - design overlay 消费 pointer。
   - emit `begin` patch。
   - drag emit `change` patch。
   - release emit `end` patch。
   - 禁止同一事件触发 pan、reference drag、pixel sample。

2. 如果按下位置命中 locked handle：
   - 只选择该点。
   - 不 emit patch。
   - 不触发 pan/reference drag/pixel sample。

3. 如果 click 命中可编辑 handle 且不是 drag：
   - 选择点。
   - 打开 color popover。

4. 如果 click 空白区域：
   - 关闭 color popover。
   - 不 pan。
   - 不 sample pixel。

结论：design mode 下 primary button 不再用于 viewport pan。需要 pan 时使用 middle drag 或 scroll/pinch。

### Pointer Middle Button

- middle drag 保持 viewport pan。
- 这也是 design mode 下主要平移手段。
- 如果未来需要触控板无中键支持，再考虑 `Space + primary drag`，本迁移不引入新 shortcut。

### Wheel / Trackpad

- scroll pan 保持。
- pinch/zoom 保持。
- `ApplyScrollPan` 和 `ApplyZoomAroundPointer` 不被 design session 禁用。

### Canvas Context Menu

- pass dep tree 的右键 `Design` 入口不变。
- canvas 自己的右键菜单保留 `复制材质`。
- design active 时 canvas 右键菜单增加 `Exit Design`。
- 右键不改变 selected point，不打开 color popover。

### Reference Image

Design mode 下 reference image 状态保留，但不允许 primary drag reference。

推荐第一版显示策略：

- Design mode 显示 raw target texture。
- 暂时隐藏 reference overlay/diff display/clipping/qualifier/pixel-value overlay。
- 这些开关状态不改变，退出 design 后恢复原显示效果。

原因：MeshGradient 编辑需要看到真实 target output；分析 overlay 会遮挡颜色和控制点判断。

### Preview Texture

Design session 需要临时 preview target texture，但不能走现有 `SetPreviewTexture`，因为它会 reset viewport。

实现建议：

- 新增 reducer helper：`set_preview_texture_without_reset(...)`。
- 进入 design 时记录 `previous_preview_texture`。
- 切换 design target 时只替换 target texture，不覆盖最初的 previous preview。
- 退出 design 时恢复 previous preview；如果进入前没有 preview，则清空 preview。
- 如果 target texture 不存在或被 scene rebuild 移除，session 进入 stale 状态，不自动退出。

### Pixel Sampling

- Design active 时禁用 `maybe_sample_clicked_pixel(...)`。
- Pixel value overlay 也隐藏。
- 以后如果需要，可加 modifier-based sampling；本迁移不做。

### Interaction Events Sent Back To Editor

`interaction_bridge` 当前在 preview/reference active 时不会发送 clean canvas pointer/key events。Design session 也应纳入 dirty interaction state：

```rust
let interaction_clean_state = !has_preview_texture
    && !has_reference_compare
    && !app.canvas.design.active.is_some();
```

即使某些 stale design session 没有 preview texture，也不能把 design click/drag 作为普通 scene interaction 发给 editor。

### Shortwire / Pass Debug

Shortwire owns canvas reference/diff paste behavior. Design session 和 active shortwire 不能同时编辑 canvas。

规则：

- 如果 `pass_debug_window::has_active_shortwire(...)` 为 true，`OpenPassDesign` no-op，并记录 debug log/status。
- 如果 design session active 后用户进入 shortwire，退出 design session。
- 普通 pass debug window 打开但 shortwire inactive 时，允许 design session；construction border 可以继续显示。

### Matrix Test Mode

Matrix mode canvas 显示的是 grid，不是单个 pass target。Design session 第一版不支持 matrix mode。

规则：

- 如果 `app.shell.test_mode == TestMode::Matrix`，`OpenPassDesign` no-op，并记录 debug log/status。
- 不自动退出 matrix mode，避免隐藏用户正在看的测试状态。

### Timeline Hover Preview

Timeline hover 会临时修改 uniform scene 和 render output。Design session active 时不应该被 timeline hover 改变正在编辑的 target semantics。

规则：

- Design active 时，timeline hover 仍可更新 scene uniforms。
- 如果 target pass 仍存在，overlay 读取更新后的 `uniform_scene`。
- 如果 hover 导致 pass/node 不存在或类型变化，显示 stale badge，不发 patch。

### Scene Update / Rebuild

每帧从最新 `resource_snapshot` 同步 target metadata：

- target pass still present。
- target texture name still valid。
- target size refreshed。
- node id/type still match scene。

失效状态：

- editor disconnected。
- scene missing。
- node missing。
- node type changed。
- pass missing。
- target texture missing。

失效时：

- 画 stale badge。
- 不响应 handle drag/color edit。
- `Esc` 仍退出。

## Implementation Steps

1. Add canvas design state.
   - `CanvasDesignState`
   - `CanvasDesignSession`
   - `CanvasDesignToolState`
   - `MeshGradientDesignState`
   - `CanvasState.design`
   - reducer actions: `EnterPassDesign`, `ExitPassDesign`

2. Add design registry and enum dispatch.
   - `tool_kind_for_target(...)`
   - `enter_session(...)`
   - `show_active_overlay(...)`
   - controller-neutral `DesignOverlayInput` / `DesignOverlayOutput`

3. Move MeshGradient design logic.
   - Extract reusable value parsing and rendering from old pass design window code.
   - Put it under `src/app/canvas/design/mesh_gradient.rs`.
   - Keep it window-free; no `show_viewport_deferred`, no native texture registration per design window.

4. Change command dispatch.
   - `AppCommand::OpenPassDesign(target)` -> canvas `EnterPassDesign(target)`.
   - Guard shortwire active and matrix mode.

5. Integrate presenter.
   - Render design overlay after target texture.
   - Return generated `DesignParamPatchPayload`s through `CanvasFrameResult.commands`.
   - Use `DesignInteractionClaims` to disable pixel sampling/reference drag/analysis overlays.

6. Adjust display/analysis behavior.
   - Ensure design mode shows raw target preview.
   - Suppress reference/diff/clipping/qualifier/pixel overlay while active without mutating their stored settings.

7. Adjust interaction bridge.
   - Treat design session as non-clean rendering state.

8. Remove window implementation.
   - Delete pass design window module and all shell/present references.

9. Tests and verification.

## Verification

Manual:

- Right-click MeshGradient pass in dependency tree -> `Design`.
- Canvas switches to target texture without changing current zoom/pan.
- Drag an unlocked handle -> editor node params update and render updates.
- Drag locked handle -> no patch emitted.
- Click unlocked handle -> color popover opens and color changes patch editor params.
- Click empty canvas -> popover closes, viewport does not pan.
- Middle drag and scroll zoom still work.
- `Esc` exits design and restores previous preview.
- Existing canvas context menu still copies material; design active context menu can exit design.
- Shortwire active -> `Design` does not enter session.
- Matrix mode active -> `Design` does not enter session.
- Scene change removing target node/pass shows stale badge and does not emit patch.

Focused automated checks:

```bash
cargo test --test render_cases mesh-gradient
cargo test design_param_patch_payload_serializes_with_editor_field_names
cargo test connected_mesh_gradient_ports_are_locked
```

Add or move unit tests for:

- design param allowlist.
- locked MeshGradient ports.
- target sync from resource snapshot.
- preview restore on exit without viewport reset.
- interaction clean-state returns false while design session active.
- registry returns MeshGradient only for MeshGradient targets.
- future tool variants can be added without modifying common session fields.

## Open Follow-Ups

- Touch editing is out of scope for this migration.
- Reference overlay while designing can be revisited after the raw target editing flow is stable.
- A future first-class status/toast system could replace debug logs for no-op entry cases.
