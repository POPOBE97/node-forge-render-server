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

> 更新：目前默认渲染已切到“VM Shader（固定 WGSL）+ storage buffers”，不再使用“按节点拼 WGSL”。

- VM WGSL：`src/shaders/bytecode_vm.wgsl`
  - 固定不变的解释器 shader
  - 从 storage buffer 读取 `globals/program/consts` 并在 fragment 中解释执行
- （历史）UBO WGSL：`src/shaders/fixed_ubo_rect.wgsl`
  - 旧方案：固定 WGSL + per-pass UBO
  - 目前已被 VM 方案替代，可作为参考保留

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
      - 使用 `src/shaders/bytecode_vm.wgsl`（固定）
      - 绑定几何 vertex buffer
      - 绑定 color attachment 为 target texture
      - 绑定 3 个 storage buffer（见 4.3）
      - 清屏透明，blend 用 `REPLACE`

- `CompositeOutput`
  - 本实现把“最终显示的输出纹理”定义为：
    - **最后一个可达 RenderPass 的 target RenderTexture**
  - 在 `eframe::App::update` 的首帧把该纹理注册为 egui `TextureId`，之后复用

  ---

  ## 4.3 VM Shader（参数表 + 指令序列）执行模型

  为什么要 VM？

  - 不再“按节点拼 WGSL”，避免 shader/pipeline 频繁重建
  - 节点与参数保留在 CPU，实时更新/动画只需要更新 buffer
  - 更通用：CPU 可以把任意材质子图编译成 bytecode（program）+ 常量表（consts）

  ### 4.3.1 绑定的 3 个 buffer

  每个 `RenderPass` 创建并绑定以下 3 个 **storage buffer**（而不是 UBO）：

  1) `globals_<passId>`（binding 0）
    - 结构：`scale/time/prog_len/const_len` 等（`Globals`）
    - 每帧更新：只更新 `time`（示例里用于动画）
  2) `program_<passId>`（binding 1）
    - `u32` 数组
    - VM 以 `word0 + imm`（两 u32）为一条指令读取
  3) `consts_<passId>`（binding 2）
    - `vec4f` 常量表（Rust 侧用 `[f32;4]`）

  Rust 侧命名规则：

  - `globals_<RenderPassNodeId>`
  - `program_<RenderPassNodeId>`
  - `consts_<RenderPassNodeId>`

  ### 4.3.2 指令格式（当前最小集）

  在 `program` 里，每条指令占 2 个 u32：

  - `word0`: 打包字段
    - `op`  : bits 0..7
    - `dst` : bits 8..15
    - `a`   : bits 16..23
    - `b`   : bits 24..31
  - `imm`: 立即数（例如常量索引）

  寄存器：`vec4f regs[16]`。

  目前实现的 opcode：

  - `OP_LOAD_CONST (1)`: `regs[dst] = consts[imm]`
  - `OP_UV (2)`: `regs[dst] = vec4f(uv, 0, 1)`（uv 由 vertex position 合成）
  - `OP_MUL (3)`: `regs[dst] = regs[a] * regs[b]`
  - `OP_ADD (4)`: `regs[dst] = regs[a] + regs[b]`
  - `OP_SIN_TIME (5)`: `regs[dst] = vec4f(k,k,k,1)`，其中 `k = 0.6 + 0.4*sin(time)`
  - `OP_OUTPUT (255)`: 输出 `regs[a]` 并结束

  ### 4.3.3 目前 CPU 侧的“编译器”做了什么

  当前只是最小 demo 编译：

  - 如果 `RenderPass.material` 连到 `Attribute`：生成 `program_uv_debug()`
  - 否则：生成 `program_constant_animated()`（常量色 * sin(time)）

  你后续可以把完整 material 子图（从 `RenderPass.material` 向上游回溯）编译成：

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
- `Attribute -> material` 目前只支持“UV debug”这一个最小语义（作为 VM 编译示例）
- 未实现端口类型兼容检查、端口存在性检查（只做了“必须存在关键入边”的硬性校验）
- 使用 `Box::leak` 处理 name 的 'static 生命周期（适用于 demo，不适合频繁 hot-reload）
- VM 指令集极小（只够验证“参数表 + bytecode + 动画 time”链路）

---

## 7. 扩展建议（下一步怎么做）

如果要逐步变成真正的「Node Forge 渲染端」：

1) 完善校验（参考 docs/render-server-dsl-parsing.md 第 9 节）
   - 端口存在性 + 类型兼容
   - 必填输入检查

2) VM 指令集扩展（推荐优先级）
  - 常量与基础算子：Float/Vec2/Vec3/Vec4、Add/Mul/Clamp/Mix
  - 时间与动画：Time、Sin/Cos、Smoothstep
  - 纹理采样：Texture2D + Sampler（需要补 texture/sampler bindings）

3) DSL → Bytecode 编译器
  - 从 `RenderPass.material` 往上游回溯材质子图
  - 生成 `consts`（常量池）与 `program`（指令序列）
  - 参数变化：只更新 buffers；结构变化：重建一次 pipeline/prepare

4) Scene 更新
   - 接入 WebSocket `scene_update`（未来再把“构建 + prepare”变成可重入/可替换）

---

## 8. 依赖

`Cargo.toml` 额外增加：
- `serde` / `serde_json`：DSL JSON 反序列化
- `anyhow`：错误处理
