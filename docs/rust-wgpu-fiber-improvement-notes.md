# rust-wgpu-fiber 改进建议（来自 node-forge-render-server 实战）

日期：2026-01-06

这份笔记基于我在一个“DSL 驱动 + 可热更新（program/consts/globals）+ egui 预览”的项目中使用 rust-wgpu-fiber 的体验总结。
目标不是追求“更全功能”，而是让 **增量更新、动态资源、错误可诊断、生命周期更自然**。

## 0. 背景与使用方式

- 典型流程：
  - 初始化：声明 buffer/texture/render_pass/composite
  - `prepare()`：创建 wgpu 资源 + pipeline
  - 运行时：每帧 `render()`，并用 `queue.write_buffer(...)` 更新少量数据
- 本项目为了避免“按节点拼 WGSL”，采用固定 WGSL（一个 bytecode VM），CPU 侧生成 program/consts，GPU 解释执行；每帧只更新 globals.time。

## 1. 主要痛点（真实遇到的摩擦）

### 1.1 资源命名强依赖 `'static`，导致 `Box::leak`

- builder API 里资源名常用 `&'a str`，而内部又需要长期保存。
- 在应用层最简单的做法是把 nodeId `String` 变成 `&'static str`（`Box::leak`），这对 demo 可接受，但对“WebSocket 串流 + 频繁 scene 重载”会变成实质内存泄露风险。

**建议**：库侧提供“拥有型 name”能力，避免用户自行泄露字符串。

### 1.2 缺少“运行时更新接口”导致用户绕开抽象层

- `ShaderSpace` 在 `prepare()` 之后，用户往往需要：
  - 更新某个 pass 的某个 buffer（globals/program/consts），或者更新 texture
  - 查询某个 buffer 的容量 / size，以决定是否能原地更新
- 现在通常只能：
  - 直接锁 `shader_space.buffers` 找到内部结构，再拿 `wgpu_buffer` 写
  - 这会让应用代码与内部数据结构强耦合（未来 rwf 内部结构变更会破坏下游代码）

**建议**：提供稳定的 runtime API（见 §3）。

### 1.3 builder 闭包模型对“动态构建”不友好

- `buffer_pool(|builder| { ...; builder })` / `texture_pool(...)` 这种链式 builder + closure，
  在复杂场景（从 DSL 扫描生成 N 个资源）时很容易遇到：
  - closure 捕获局部变量导致生命周期/所有权不直观
  - 需要为了 Rust 借用规则提前“预计算 specs”，增加样板代码

**建议**：除了 closure builder 以外，提供一种更“数据驱动”的批量声明方式（例如传 Vec<Spec>）。

### 1.4 缺少“变更类型”的官方路径：参数变更 vs 结构变更

真实产品里，scene 更新通常分两类：
- **参数变更**：program/consts/globals 更新，不需要重建 pipeline
- **结构变更**：新增/删除 pass、改变 attachment/format、改变 bind group layout，需要重建

目前库层面没有明确的“增量更新边界”，应用层只能自己约定。

**建议**：库侧抽象出 `RuntimeUpdate` 或 `Patch` 概念（见 §4）。

### 1.5 错误信息、调试能力与可观测性不足

- 常见问题：资源名拼错、未准备、绑定不匹配、纹理没创建成功等。
- 如果能更快定位“哪个 pass / 哪个绑定 / 哪个资源名”会省大量时间。

**建议**：更结构化的错误类型 + debug dump（见 §5）。

## 2. 低风险、高收益的 API 改进（优先级最高）

### 2.1 资源名支持 owned（解决 `'static` 泄露）

**目标**：用户可以用 `String`/`Cow<'a, str>` 定义资源名；库内部把 name 存进 arena/interner。

候选 API：
- `ResourceName`：
  - `impl From<&str>` / `impl From<String>`
  - 内部用 `Arc<str>` 或 `SmolStr`（取决于性能/依赖策略）
- ShaderSpace 内部维护 `NameInterner`：把用户传入的 name 归一化到稳定句柄。

兼容策略：保留现有 `&str` API，同时新增 `*_owned(...)` 或泛型 `Into<ResourceName>`。

### 2.2 公开稳定的“按名/按句柄写入 buffer/texture”

**目标**：用户不需要锁内部 HashMap，也不依赖内部 Fish/BufferEntry 结构。

候选 API：
- `shader_space.write_buffer(name, offset, bytes) -> Result<()>`
- `shader_space.write_buffer_typed<T: Pod>(name, &T) -> Result<()>`
- `shader_space.write_buffer_slice<T: Pod>(name, &[T]) -> Result<()>`
- `shader_space.copy_texture_to_texture(...)`（如果支持离屏链路调试/输出）

同时返回错误时包含：name、期望大小/实际大小、是否 prepared。

### 2.3 提供资源查询：size/usage/format/是否 prepared

**目标**：热更新必须知道容量，避免 silent overflow。

候选 API：
- `shader_space.buffer_info(name) -> Option<BufferInfo { size, usage, prepared }>`
- `shader_space.texture_info(name) -> Option<TextureInfo { size, format, usage, prepared }>`

这类信息是稳定的“外部契约”，不需要暴露内部实现细节。

## 3. 面向“热更新/串流”的建议设计

### 3.1 给 RenderPass 生成“bindings 句柄”

**目标**：用户构建 pass 时能拿到该 pass 的关键绑定信息，后续更新只需要句柄。

候选 API：
- `let pass = shader_space.add_render_pass(...);`
- `pass.bind_storage_buffer(slot, name, stages, readonly);`
- `let bindings = pass.bindings_handle(); // 包含 globals/program/consts 名称或句柄`

这样应用层不需要自行维护 `globals_{pass_id}` 之类的字符串拼接。

### 3.2 分离“声明期”与“运行期”对象

现在 `ShaderSpace` 同时承担：
- 声明资源/构建 pipeline
- 运行时渲染/更新

建议拆成：
- `ShaderSpaceBuilder`：声明期，收集 spec
- `ShaderSpaceRuntime`：`prepare()` 产物，专注 render + update + query

这能让 API 更清晰，也能更容易实现“结构变更重建”策略。

## 4. 结构变更的 Patch/重建策略

建议定义清晰的更新路径：

- `apply_patch(patch: ShaderSpacePatch) -> Result<PatchResult>`
  - 如果 patch 仅影响 buffer contents：返回 `PatchResult::Applied`
  - 如果 patch 影响资源大小/format/layout：返回 `PatchResult::RequiresRebuild { reason }`

这样 WebSocket 串流更新可以做到：
- 快路径：只更新 program/consts/globals
- 慢路径：触发 rebuild（并能给出原因）

## 5. 调试/诊断/可观测性

### 5.1 Debug dump

- `shader_space.dump_graphviz()`：导出 pass/attachment/binding 关系
- `shader_space.dump_summary()`：列出所有资源名、类型、size/format、prepared 状态

### 5.2 结构化错误

把常见错误做成 enum，带字段：
- `MissingResource { kind, name }`
- `NotPrepared { kind, name }`
- `SizeOverflow { name, need, capacity }`
- `BindMismatch { pass, bind_group, binding, expected, actual }`

并实现 `Display` 输出可读信息。

## 6. 文档与示例建议

- 增加一个官方示例：
  - 固定 shader + 每帧更新 buffer
  - 演示“参数变更不重建”的正确姿势
- 增加一个示例：
  - scene 热重载（文件 watcher 或 websocket）
  - 明确区分“只更新 contents”与“需要 rebuild”

## 7. 兼容性与迁移建议（给维护者）

- 尽量 **增量添加**：
  - 保留现有 builder API
  - 新增 runtime update/query API
  - 新增 owned-name 支持（内部 interner），并允许旧 `&str` 无缝转入
- 把“内部 HashMap + Mutex”继续当实现细节，外部只通过稳定 API 访问。

## 8. 附：我在本项目里写过的临时 workaround（不建议库用户长期使用）

- 用 `Box::leak(String)` 把动态 nodeId 变成 `&'static str`，只为满足 builder 存名要求。
- 为了绕开 closure 捕获/借用问题，先把 DSL 扫描结果预计算成 `Vec<Spec>` 再喂给 builder。
- 运行时更新通过锁内部 `shader_space.buffers` 查找 buffer，再调用 `queue.write_buffer`。

这些 workaround 都能通过上面建议的 API 改进自然消失。
