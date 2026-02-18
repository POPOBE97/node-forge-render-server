# node-forge-render-server

一个基于 Rust + wgpu/eframe 的本地渲染进程，并通过 WebSocket 接收/返回场景（SceneDSL）JSON。

## 运行

```bash
cargo run --release
```

启动后会打开一个窗口，并在控制台打印：

- WebSocket：`ws://0.0.0.0:8080`

> 默认会从 `assets/` 读取示例场景作为初始 scene。

## 测试

- WGSL 生成测试（如何新增/更新测试用例）：见 [docs/testing-wgsl-generation.md](docs/testing-wgsl-generation.md)

## 几何与坐标推导（Refactor）

- 几何/坐标统一解析方案与语义说明：
  [docs/geometry-coordination-resolver-refactor.md](docs/geometry-coordination-resolver-refactor.md)

## UV 约定（简短）

- 内部 `in.uv` 使用 WGSL 纹理坐标：左上角为原点。
- GLSL-like 本地像素坐标使用：`local_px = vec2(uv.x, 1.0 - uv.y) * geo_size`。
- 用户可见的 `Attribute.uv` 保持 GLSL-like（左下角为原点）：`vec2(in.uv.x, 1.0 - in.uv.y)`。

## WebSocket 协议（最小集合）

所有消息统一结构：

```json
{
  "type": "scene_update | scene_request | ping | pong | error",
  "timestamp": 0,
  "requestId": "optional",
  "payload": {}
}
```

- `scene_update`: `payload` 为完整 SceneDSL JSON；服务端会尽量只保留“最新一条”更新。
- `scene_request`: `payload` 为空；服务端返回最近一次通过校验的 scene（`type=scene_update`）。
- `ping`: 服务端返回 `pong`（带原 `requestId`）。
- `error`: `payload` 为 `{ code, message }`。

## 发送场景（Node 工具）

安装依赖：

```bash
cd tools
npm install
```

发送一个 scene 文件到服务端：

```bash
node tools/ws-send-scene.js assets/node-forge-example.1.json ws://127.0.0.1:8080
```

向服务端请求最近一次 scene（会打印服务端返回的 JSON）：

```bash
node tools/ws-send-scene.js --request ws://127.0.0.1:8080
```
