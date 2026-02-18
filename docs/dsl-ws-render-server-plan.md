## Plan: WebSocket 接收 DSL 并实时渲染

把现有“本地 JSON 一次性加载 scene”的流程改造成“WS 持续接收 scene_update 并在 UI 渲染线程应用”。网络侧只负责接收/解析/轻校验并把 `SceneDSL` 推到线程安全队列；渲染侧在每帧 drain 队列，默认采用“全量重建 `ShaderSpace` + `prepare()`”确保正确出画面，后续再按需做增量更新优化。建议将本文保存为 `docs/dsl-ws-render-server-plan.md`。

### Steps 5
1. 定义协议与消息体：新增 `src/protocol.rs` 对齐 `WSMessage`（`type/timestamp/requestId?/payload`），`payload` 复用 `src/dsl.rs` 的 `SceneDSL`。
2. 增加 WebSocket 接收层：新增 `src/ws.rs` 实现 WS server（默认 `0.0.0.0:8080`），处理 `scene_update/scene_request/ping`，把最新 `SceneDSL` 写入 channel（不触碰渲染对象）。
3. 将“scene 来源”接入应用主循环：在 `src/main.rs` / `src/app.rs` 初始化时创建 receiver，把它挂到 `NodeForgeApp`（或复用 `src/stream.rs` 的 `SceneSource` seam 实现 `WebSocketSceneSource`）。
4. 在每帧应用最新 scene：在 `NodeForgeApp::update`（见 `src/app.rs`）non-blocking drain receiver，拿到最新 `SceneDSL` 后调用 `src/dsl.rs` 的 `build_shader_space(...)` 全量重建 `ShaderSpace` 并触发重新 `prepare()`（见 `src/renderer/shader_space/`），确保画面立即更新。
5. 增加最小校验与错误回传：复用现有构建阶段的硬校验（例如 `Composite.target <- RenderTexture`、draw pass geometry 可解析等），失败时按协议回 `error`（`PARSE_ERROR/VALIDATION_ERROR`），并保留 last-good scene 继续渲染。

### Further Considerations 3
1. WS 依赖选型：`tokio + tokio-tungstenite`（轻量）或 `axum`（未来扩展 HTTP/鉴权更方便）？
2. 更新策略：先全量重建（简单可靠）还是增加“结构变更才重建、参数变更走 `update_globals`”的 diff（见 `src/dsl.rs` / `src/renderer/shader_space/api.rs`）？
3. 渲染端行为约束：Editor 300ms debounce 高频更新，建议 receiver 只保留最新一份 scene（丢弃旧的）以避免堆积与卡顿。
