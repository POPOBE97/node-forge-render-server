# node-forge-render-server

一个基于 Rust + wgpu/eframe 的本地渲染进程，并通过 WebSocket 接收/返回场景（SceneDSL）JSON。

## 运行

```bash
cargo run --release
```

启动后会打开一个窗口，并在控制台打印：

- WebSocket：`ws://0.0.0.0:8080`

> 默认会从 `assets/` 读取示例场景作为初始 scene。

## Pixel Perfecto

显微镜照片的 Diamond RGB 子像素网格对齐工具，支持自动初对齐、单画布累积校准、图片旋转、拖格点建立标定锚点、自动补点及逐图透视／镜头矫正，也支持横纵标尺与共享辅助线、两点数像素（图像／网格独立计数与差值）、吸附拖动、叠加／差值／A/B／卷帘比较，以及打包工程和 PNG 导出。

```bash
cargo run --bin pixel-perfecto -- photo-a.png photo-b.jpg
```

也可从 Render Server 侧栏的 **Pixel Perfecto** 按钮打开独立工具窗口。完整操作说明见 [docs/pixel-perfecto.md](docs/pixel-perfecto.md)。

## Camera Studio

Canon T4i / 650D 拍摄工具，提供 Live View、相机参数、手动步进对焦、照片自动下载及相机录像后的 MOV 下载。
从 Render Server 侧栏 **Camera Studio** 打开原生工具窗口，也可以独立启动：

```bash
brew install libgphoto2 gphoto2
cargo run --bin camera-studio
```

原始 JPEG/CR2/MOV 与拍摄记录保存在本地目录。安装、恢复下载和实机验证状态见 [docs/camera-studio.md](docs/camera-studio.md)。

Camera Studio 的 **Screen test** 提供独立的几何标定、iPhone RAW 参考库和安卓色彩检测。手机使用原生播放器，通过 USB 显示按屏幕分辨率生成的测试图；操作、构建和验收边界见 [docs/screen-test.md](docs/screen-test.md)。

## Android 截图到剪贴板

连接一台已授权 USB 调试的 Android 设备后，可直接截图并写入系统剪贴板。默认执行 Display P3 到 sRGB 的像素转换，适合粘贴到 Figma：

```bash
node-forge-render-server --android-screencap-clipboard
```

保留 Android `screencap` 返回的原始 PNG 与色彩配置：

```bash
node-forge-render-server --android-screencap-clipboard source
```

该命令执行一次后立即退出；设备必须是唯一一台处于 `device` 状态的 USB 设备。

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
  "type": "scene_update | scene_request | ping | pong | error | interaction_event",
  "timestamp": 0,
  "requestId": "optional",
  "payload": {}
}
```

- `scene_update`: `payload` 为完整 SceneDSL JSON；服务端会尽量只保留“最新一条”更新。
- `scene_request`: `payload` 为空；服务端返回最近一次通过校验的 scene（`type=scene_update`）。
- `ping`: 服务端返回 `pong`（带原 `requestId`）。
- `error`: `payload` 为 `{ code, message }`。
- `interaction_event`: 服务端从 Canvas 交互侧广播的输入事件报告（仅在 clean 渲染态：无 texture preview 且无 reference compare 时发送）。
  - `payload.eventType`: `keydown | keyup | mousedown | mouseup | mousemove | wheel | touchstart | touchmove | touchend | touchcancel`
  - `payload.seq`: 单调递增事件序号
  - `payload.data`: 可选事件数据（按事件类型出现）

`interaction_event` 示例：

```json
{
  "type": "interaction_event",
  "timestamp": 1730000000000,
  "payload": {
    "eventType": "mousedown",
    "seq": 42,
    "data": {
      "position": {
        "clientX": 512.0,
        "clientY": 288.0,
        "canvasX": 128.0,
        "canvasY": 64.0
      },
      "button": "left",
      "modifiers": {
        "alt": false,
        "ctrl": false,
        "shift": false,
        "meta": false
      }
    }
  }
}
```

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
