# DSL → rust-wgpu-fiber 渲染骨架（本仓库实现说明）

本文档记录当前仓库里“读取 Node Forge DSL JSON（示例文件）→ 解析成计算图 → 映射到 rust-wgpu-fiber ShaderSpace → 在 eframe/egui 里显示最终 RenderTexture”的最小实现。

目标：让你在 Rust 侧先跑通 **DSL 解析 + 拓扑执行 + 可视化输出纹理** 这条链路，然后再逐步把材质/着色器/更多节点类型补齐。

---

## 1. 入口与文件

- 入口：`src/main.rs`
  - 负责读取 `assets/node-forge-example.1.json`
  - 负责 DSL 解析/拓扑排序/可达子图裁剪
  - 负责用 `rust_wgpu_fiber::shader_space::ShaderSpace` 构建资源与 pass
  - 负责 eframe/egui 窗口与纹理显示
- 最小 WGSL：`src/shaders/simple_rect.wgsl`
  - 当前仅输出纯色（用于验证管线与 attachment 写入正确）

> 更新：目前默认渲染已切回“按节点/材质子图拼 WGSL → 为每个 RenderPass 生成专用 shader”的方案。

- （参考保留）VM WGSL：`src/shaders/bytecode_vm.wgsl`
  - 旧实验：固定解释器 shader + storage buffers
  - 当前默认路径不再使用（保留文件用于对比/后续研究）
- （参考保留）UBO WGSL：`src/shaders/fixed_ubo_rect.wgsl`
  - 固定 WGSL + per-pass uniform 的参考版本

---

## 2. DSL 解析策略（对齐 docs/render-server-dsl-parsing.md 的核心点）

本实现只用到该示例 DSL 的最小子集：

- Node types：
  - `Rect2DGeometry`
  - `RenderTexture`
  - `RenderPass`
  - `CompositeOutput`
  - `Attribute`（示例里存在，但当前实现不参与渲染逻辑）
- Connections：用于把 `RenderPass.geometry/target` 以及 `CompositeOutput.image` 串起来

### 2.1 数据结构

在 `src/main.rs` 定义了最小反序列化结构：

- `SceneDSL { version, metadata, nodes, connections, outputs }`
- `Node { id, node_type, params }`
- `Connection { from, to }`

说明：
- `params` 使用 `HashMap<String, serde_json::Value>`，目前只读取 `RenderTexture.width/height/format`

### 2.2 解析输入来源

- 直接从 `assets/node-forge-example.1.json` 读取
- 用 `serde_json::from_str` 解析

### 2.3 输入解析优先级（Inline inputs / defaults）

当前实现**只在 RenderTexture 上读取 inline inputs**：
- `width` / `height` / `format`

其它节点（例如 Rect2DGeometry.params.width）暂未生效；如果要严格遵循「连线优先 → inline → default」策略，需要在后续扩展节点执行时补上。

---

## 3. 图构建与执行顺序

### 3.1 拓扑排序

- 在 `src/main.rs` 里实现 `topo_sort(scene)`：
  - 以 connection 的 `from.nodeId -> to.nodeId` 构建 DAG
  - Kahn 算法计算拓扑序
  - 若存在环，直接报错

### 3.2 可达子图裁剪（从输出回溯）

为了避免执行无关节点，本实现从最终输出节点回溯所有上游节点：

- 输出节点选择：
  1) `outputs.composite`（若存在）
  2) 否则扫描 `nodes` 里第一个 `CompositeOutput`

- 在示例中：
  - `CompositeOutput.image` 的入边来自某个 `RenderPass.pass`

- `upstream_reachable(scene, outputNodeId)`：
  - 根据 `to.nodeId -> from.nodeId` 的反向邻接表深搜/栈遍历
  - 得到 `HashSet<String>` 的可达 nodeId

### 3.3 本实现最终会执行哪些 pass？

- 在拓扑序里筛选：
  - `reachable` 内
  - 且 node_type == `RenderPass`

这会得到一个按拓扑顺序排列的 `RenderPass` 列表，并用于 `shader_space.composite`。

---

## 4. rust-wgpu-fiber 映射规则

> rust-wgpu-fiber 的用法与 docs/rust-wgpu-fiber.md 一致：初始化阶段声明资源与 pass，调用一次 `prepare()`，每帧只 `render()`。

### 4.1 关键点：名字生命周期（'static）

`ShaderSpace` 的 builder API（`buffer_pool/texture_pool/render_pass/composite`）内部会保存你传入的资源名/ pass 名。

为了避免把局部 `String` 引用捕获进 closure 导致生命周期问题，本实现采取了**最小粗暴**做法：

- 把 DSL 的 nodeId 字符串 `String` 用 `Box::leak` 转成 `&'static str`
- 在内存里永久存在（进程结束前不释放）

这在 demo/工具场景里很方便；如果你要长期运行并频繁更新 scene，需要替换为更严谨的名称管理策略。

### 4.2 节点到 ShaderSpace

- `Rect2DGeometry`
  - `buffer_pool` 里：`plane_geometry(nodeId)`
  - 目前忽略 `params.width/height`

- `RenderTexture`
  - `texture_pool` 里：`texture(nodeId, [w,h], format, usages)`
  - usages 包含：
    - `RENDER_ATTACHMENT`（可写入）
    - `TEXTURE_BINDING`（给 egui 采样显示）
    - `COPY_SRC`（后续如需截帧/读回可用）

- `RenderPass`
  - 对每个 pass：
    - 解析 `RenderPass.geometry` 入边：必须来自 `Rect2DGeometry`
    - 解析 `RenderPass.target` 入边：必须来自 `RenderTexture`
    - `render_pass(passId, |builder| ...)`：
      - 运行时生成该 pass 的 WGSL（根据 `RenderPass.material` 上游节点）
      - 绑定几何 vertex buffer
      - 绑定 color attachment 为 target texture
      - 绑定 1 个 uniform buffer：`params_<passId>`（scale/time/color）
      - 清屏透明，blend 用 `REPLACE`
---
  - `consts`: 常量池（颜色、float、vec2/vec3/vec4 等统一装进 vec4）
  - `program`: 指令序列（后序遍历/拓扑遍历生成）

  ---

---

## 5. 运行方式

在仓库根目录执行：

```bash
cargo run
```

预期结果：
- 会弹出一个窗口
- 画面是纯色矩形（来自 `simple_rect.wgsl`）

---

## 6. 当前限制（故意保持最小）

- 只支持示例里的最小节点集（Rect2DGeometry / RenderTexture / RenderPass / CompositeOutput）
- `Attribute -> material` 目前只支持“UV debug”这一个最小语义
- 未实现端口类型兼容检查、端口存在性检查（只做了“必须存在关键入边”的硬性校验）
- 使用 `Box::leak` 处理 name 的 'static 生命周期（适用于 demo，不适合频繁 hot-reload）
- 材质节点集极小（只够验证“按材质子图生成 WGSL + time 动画”链路）

---

## 7. 扩展建议（下一步怎么做）

如果要逐步变成真正的「Node Forge 渲染端」：

1) 完善校验（参考 docs/render-server-dsl-parsing.md 第 9 节）
   - 端口存在性 + 类型兼容
   - 必填输入检查

2) WGSL 生成器扩展（推荐优先级）
  - 常量与基础算子：Float/Vec2/Vec3/Vec4、Add/Mul/Clamp/Mix
  - 时间与动画：Time、Sin/Cos、Smoothstep
  - 纹理采样：Texture2D + Sampler（需要补 texture/sampler bindings）

3) DSL → fragment WGSL（材质子图编译）
  - 从 `RenderPass.material` 往上游回溯材质子图
  - 生成一段 `fs_main`（以及必要的 helper 函数）
  - 参数变化：只更新 uniform；结构变化：重建一次 pipeline/prepare

4) Scene 更新
   - 接入 WebSocket `scene_update`（未来再把“构建 + prepare”变成可重入/可替换）

---

## 8. 依赖

`Cargo.toml` 额外增加：
- `serde` / `serde_json`：DSL JSON 反序列化
- `anyhow`：错误处理
