# DSL → rust-wgpu-fiber 渲染实现文档

本文档描述当前仓库如何将 Node Forge DSL JSON 解析并渲染：

**DSL JSON → 图解析/验证 → WGSL 着色器生成 → rust-wgpu-fiber ShaderSpace → eframe/egui 窗口显示**

---

## 1. 架构概览

### 1.1 核心模块

| 文件 | 职责 |
|------|------|
| [src/main.rs](../src/main.rs) | 程序入口，初始化 eframe 窗口，构建 ShaderSpace |
| [src/app.rs](../src/app.rs) | eframe App 实现，处理 WebSocket 场景更新，每帧渲染循环 |
| [src/dsl.rs](../src/dsl.rs) | DSL 数据结构定义，JSON 解析，默认参数合并 |
| [src/graph.rs](../src/graph.rs) | 拓扑排序，上游可达节点计算 |
| [src/schema.rs](../src/schema.rs) | 节点类型 scheme 加载，场景验证，端口类型检查 |
| [src/renderer.rs](../src/renderer.rs) | 核心渲染逻辑：场景准备、WGSL 生成、ShaderSpace 构建 |
| [src/ws.rs](../src/ws.rs) | WebSocket 服务器，接收实时场景更新 |

### 1.2 数据流

```
┌─────────────────┐      ┌─────────────────┐      ┌─────────────────┐
│  DSL JSON 文件   │ ───► │   dsl.rs 解析    │ ───► │  schema.rs 验证  │
│  或 WebSocket    │      │  + 默认参数合并   │      │  端口类型检查     │
└─────────────────┘      └─────────────────┘      └─────────────────┘
                                                           │
                                                           ▼
┌─────────────────┐      ┌─────────────────┐      ┌─────────────────┐
│  eframe 窗口渲染 │ ◄─── │   ShaderSpace   │ ◄─── │  renderer.rs    │
│  egui 纹理显示   │      │   prepare/render │      │  WGSL 生成       │
└─────────────────┘      └─────────────────┘      └─────────────────┘
```

---

## 2. DSL 数据结构

### 2.1 核心类型 (\`src/dsl.rs\`)

```rust
pub struct SceneDSL {
    pub version: String,
    pub metadata: Metadata,
    pub nodes: Vec<Node>,
    pub connections: Vec<Connection>,
    pub outputs: Option<HashMap<String, String>>,
}

pub struct Node {
    pub id: String,
    pub node_type: String,               // 节点类型，如 "RenderPass"
    pub params: HashMap<String, Value>,  // 节点参数
    pub inputs: Vec<NodePort>,           // 输入端口（用于 Composite 层级排序）
}

pub struct Connection {
    pub id: String,
    pub from: Endpoint,  // { node_id, port_id }
    pub to: Endpoint,
}
```

### 2.2 参数解析工具

- \`parse_u32(params, key)\` - 解析整数参数
- \`parse_f32(params, key)\` - 解析浮点参数
- \`parse_str(params, key)\` - 解析字符串参数
- \`parse_texture_format(params)\` - 解析纹理格式（支持 \`rgba8unorm\`, \`rgba8unormsrgb\`）

### 2.3 默认参数合并

加载 DSL 时自动从 \`assets/node-scheme.json\` 读取 \`defaultParams\` 并合并到节点参数：

```rust
fn apply_node_default_params(scene: &mut SceneDSL, scheme: &NodeScheme) {
    for node in &mut scene.nodes {
        let node_scheme = scheme.nodes.get(&node.node_type)?;
        // 合并：scheme defaults ← node params（node 优先）
        let mut merged = node_scheme.default_params.clone();
        for (k, v) in node.params {
            merged.insert(k, v);
        }
        node.params = merged;
    }
}
```

---

## 3. 节点类型 Scheme (\`src/schema.rs\`)

### 3.1 Scheme 格式

支持两种 JSON 格式（自动识别）：

**Generated 格式**（当前使用）：
```json
{
  "schemaVersion": 1,
  "nodes": [
    {
      "type": "RenderPass",
      "category": "Pass",
      "inputs": [{ "id": "material", "type": "material" }],
      "outputs": [{ "id": "pass", "type": "pass" }],
      "defaultParams": { "blendMode": "normal" }
    }
  ]
}
```

### 3.2 端口类型

端口类型支持单类型或多类型：

```rust
pub enum PortTypeSpec {
    One(String),        // 单类型：如 "pass"
    Many(Vec<String>),  // 多类型：如 ["color", "float", "vector4"]
}
```

内置端口类型：\`any\`, \`bool\`, \`color\`, \`float\`, \`geometry\`, \`int\`, \`material\`, \`pass\`, \`renderTexture\`, \`shader\`, \`texture\`, \`vector2\`, \`vector3\`

### 3.3 场景验证

\`validate_scene_against()\` 执行以下检查：

1. **节点类型存在性** - 所有节点的 \`node_type\` 必须在 scheme 中定义
2. **必填参数检查** - 验证 \`params.required == true\` 的参数存在
3. **参数类型检查** - 验证参数值符合声明的类型
4. **连接端口存在性** - \`from.portId\` 必须在输出端口中，\`to.portId\` 必须在输入端口中
5. **端口类型兼容性** - 检查连接两端的端口类型是否匹配

### 3.4 隐式类型转换

允许的隐式连接：

- **RenderPass.material 输入**：可接受 \`color\`, \`float\`, \`int\`, \`bool\`, \`vector*\` 等着色器值
- **pass 类型输入**：可接受原始着色器值，渲染器会自动合成 fullscreen RenderPass

---

## 4. 图算法 (\`src/graph.rs\`)

### 4.1 拓扑排序

```rust
pub fn topo_sort(scene: &SceneDSL) -> Result<Vec<String>>
```

- 使用 Kahn 算法
- 检测循环依赖
- 返回节点 ID 的执行顺序

### 4.2 上游可达节点

```rust
pub fn upstream_reachable(scene: &SceneDSL, start: &str) -> HashSet<String>
```

从目标节点反向遍历，收集所有上游依赖节点。用于裁剪无关子图。

---

## 5. 渲染器实现 (\`src/renderer.rs\`)

### 5.1 场景准备流程

\`prepare_scene()\` 函数：

1. **定位 RenderTarget** - 查找 \`category == "RenderTarget"\` 的节点（\`Screen\` 或 \`File\`）
2. **子图裁剪** - 只保留 RenderTarget 上游的可达节点
3. **原始值包装** - \`auto_wrap_primitive_pass_inputs()\` 将直接连接到 pass 输入的着色器值自动包装为 fullscreen RenderPass
4. **验证** - 对裁剪后的子图执行 scheme 验证
5. **拓扑排序** - 计算执行顺序
6. **输出解析** - 确定 Composite 节点和输出 RenderTexture

```rust
struct PreparedScene {
    scene: SceneDSL,
    nodes_by_id: HashMap<String, Node>,
    ids: HashMap<String, ResourceName>,
    topo_order: Vec<String>,
    composite_layers_in_draw_order: Vec<String>,
    output_texture_node_id: String,
    output_texture_name: ResourceName,
    resolution: [u32; 2],
}
```

### 5.2 WGSL 着色器生成

#### 材质表达式编译

\`compile_material_expr()\` 递归遍历材质节点图，生成 WGSL 表达式：

**支持的材质节点类型：**

| 节点类型 | 功能 | 输出类型 |
|---------|------|---------|
| \`ColorInput\` | RGBA 颜色常量 | vec4 |
| \`FloatInput\` / \`IntInput\` | 标量常量 | f32 |
| \`Vector2Input\` / \`Vector3Input\` | 向量常量 | vec2/vec3 |
| \`Attribute\` | 顶点属性（目前仅支持 \`uv\`） | vec2 |
| \`ImageTexture\` | 纹理采样 | vec4 (color) / f32 (alpha) |
| \`Time\` | 动画时间 | f32 |
| \`Float\` / \`Scalar\` / \`Constant\` | 标量常量 | f32 |
| \`Vec2\` / \`Vec3\` / \`Vec4\` / \`Color\` | 向量常量 | vec2/vec3/vec4 |
| \`Sin\` / \`Cos\` | 三角函数 | 保持输入类型 |
| \`Add\` / \`Mul\` / \`Multiply\` | 算术运算 | 自动类型推导 |
| \`Mix\` | 线性插值 | 自动类型推导 |
| \`Clamp\` | 范围限制 | 自动类型推导 |
| \`Smoothstep\` | 平滑阶跃 | 自动类型推导 |

#### 类型系统

```rust
enum ValueType { F32, Vec2, Vec3, Vec4 }

struct TypedExpr {
    ty: ValueType,
    expr: String,      // WGSL 表达式字符串
    uses_time: bool,   // 是否依赖时间（需要每帧更新）
}
```

自动类型提升：标量可以自动扩展为向量（splat）。

#### Shader Bundle 结构

```rust
pub struct WgslShaderBundle {
    pub common: String,         // 共享声明（结构体、binding）
    pub vertex: String,         // 顶点着色器模块
    pub fragment: String,       // 片段着色器模块
    pub module: String,         // 完整合并模块
    pub image_textures: Vec<String>,  // 引用的 ImageTexture 节点 ID
}
```

### 5.3 Uniform 参数

每个 RenderPass 绑定一个 \`Params\` uniform buffer：

```rust
#[repr(C)]
pub struct Params {
    pub target_size: [f32; 2],  // 目标纹理尺寸
    pub geo_size: [f32; 2],     // 几何体尺寸
    pub center: [f32; 2],       // 几何体中心偏移
    pub time: f32,              // 动画时间
    pub _pad0: f32,
    pub color: [f32; 4],        // 基础颜色
}
```

### 5.4 高斯模糊实现

\`GuassianBlurPass\` 节点使用多 pass mipmap 降采样 + 分离式高斯卷积：

1. **Mipmap 降采样** - 根据 sigma 计算所需 mip level
2. **水平/垂直卷积** - 8-tap 高斯核，分两个 pass 执行
3. **上采样** - 双线性插值回原始分辨率

---

## 6. ShaderSpace 构建

\`build_shader_space_from_scene()\` 将准备好的场景映射到 rust-wgpu-fiber：

### 6.1 资源映射

| DSL 节点 | ShaderSpace 资源 |
|---------|-----------------|
| \`Rect2DGeometry\` | Vertex buffer (6 顶点，全屏四边形) |
| \`RenderTexture\` | Texture (RENDER_ATTACHMENT + TEXTURE_BINDING + COPY_SRC) |
| \`RenderPass\` | Render pass + Uniform buffer + Pipeline |
| \`ImageTexture\` | Texture + Sampler (从 data URL 或文件加载) |

### 6.2 Composite 层级

Composite 节点支持多层混合：

- \`pass\` 输入：基础层
- \`dynamic_*\` 输入：叠加层（按 \`node.inputs\` 数组顺序绘制）

每层可配置混合模式：\`normal\`, \`add\`, \`multiply\`, \`screen\` 等。

### 6.3 混合状态解析

支持预设和自定义混合：

```json
// 预设
{ "blendMode": "normal" }

// 自定义
{
  "blendColorSrcFactor": "src-alpha",
  "blendColorDstFactor": "one-minus-src-alpha",
  "blendColorOperation": "add"
}
```

---

## 7. WebSocket 实时更新

### 7.1 服务器

默认监听 \`ws://0.0.0.0:8080\`，支持以下消息：

| 消息类型 | 方向 | 说明 |
|---------|------|------|
| \`ping\` | → | 心跳请求 |
| \`pong\` | ← | 心跳响应 |
| \`scene_request\` | → | 请求当前场景 |
| \`scene_update\` | →/← | 推送/接收场景更新 |
| \`error\` | ← | 验证/解析错误 |

### 7.2 场景热更新

\`App::update()\` 在每帧检查 \`scene_rx\` channel：

1. 解析新场景
2. 调整窗口尺寸（如果 Screen 节点分辨率变化）
3. 重建 ShaderSpace
4. 发送验证错误到客户端

---

## 8. 运行方式

```bash
# 默认加载 assets/node-forge-example.1.json
cargo run

# 使用 WebSocket 推送场景
node tools/ws-send-scene.js path/to/scene.json
```

### 8.1 错误处理

- 解析/验证失败时显示紫色错误画面
- 错误信息通过 WebSocket 返回客户端
- 保留最后一个成功的场景（\`last_good\`）

---

## 9. 支持的节点类型

### 9.1 渲染目标（RenderTarget）

- \`Screen\` - 输出到窗口
- \`File\` - 输出到文件

### 9.2 Pass 类型

- \`RenderPass\` - 标准渲染 pass
- \`GuassianBlurPass\` - 高斯模糊 pass
- \`Composite\` - 多层合成

### 9.3 资源类型

- \`Rect2DGeometry\` - 2D 矩形几何体
- \`RenderTexture\` - 渲染目标纹理
- \`ImageTexture\` - 图片纹理（支持 data URL）

### 9.4 着色器值

- 常量：\`FloatInput\`, \`IntInput\`, \`ColorInput\`, \`Vector2Input\`, \`Vector3Input\`
- 旧式常量：\`Float\`, \`Scalar\`, \`Constant\`, \`Vec2\`, \`Vec3\`, \`Vec4\`, \`Color\`
- 时间：\`Time\`
- 属性：\`Attribute\` (仅 \`uv\`)

### 9.5 数学运算

- 算术：\`Add\`, \`Mul\`/\`Multiply\`
- 函数：\`Sin\`, \`Cos\`, \`Mix\`, \`Clamp\`, \`Smoothstep\`

---

## 10. 依赖

```toml
[dependencies]
rust_wgpu_fiber = { path = "3rd/rust-wgpu-fiber" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
crossbeam-channel = "0.5"
tungstenite = "0.24"
image = "0.25"
base64 = "0.22"
```
