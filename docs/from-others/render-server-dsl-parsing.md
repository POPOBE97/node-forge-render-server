# 渲染端如何解析 Node Forge DSL（快速上手）

本文档面向“在别的地方实现 render server / renderer”的同学，目标是让你**拿到 Editor 生成的 DSL JSON 后**，能快速完成：

- 解析 JSON → 得到强类型/结构化 Scene
- 校验（schema + 基础语义）
- 构建有向图（connections）并按拓扑顺序执行
- 正确处理 inline inputs（常量）与 Input Bindings（闭包节点变量绑定）

> 术语约定：本文把 DSL 里的 `nodes` 当作“计算图节点”，把 `connections` 当作“有向边（从输出口连到输入口）”。

---

## 1. DSL 从哪里来（传输层）

在现有工程里，Editor 通过 WebSocket 发消息：

- `scene_update`：payload 是 `SceneDSL`（即本文的 DSL JSON）

消息类型定义在 [packages/server/src/protocol.ts](packages/server/src/protocol.ts)。如果你在别处实现 render server，只需要保证你接收到的 `payload` 是一个 JSON 对象（或 JSON 字符串）即可。

---

## 2. DSL 的顶层结构（SceneDSL）

核心结构定义在 [packages/dsl/src/schema.ts](packages/dsl/src/schema.ts)。你可以理解为：

```ts
interface SceneDSL {
  version: string; // 当前默认 "1.0"
  metadata: {
    name: string;
    description?: string;
    author?: string;
    created?: string;   // ISO datetime
    modified?: string;  // ISO datetime
  };
  nodes: Node[];
  connections: Connection[];
  outputs?: Record<string, string>; // 输出名 -> nodeId（可选）
}
```

### `outputs` 字段

- 该字段是可选的。
- 当前生成逻辑会在图中存在 `MaterialOutput` / `CompositeOutput` 时填充（见 [packages/dsl/src/validator.ts](packages/dsl/src/validator.ts) 的 `generateDSL`）。
- 渲染端可以：
  - **优先**使用 `outputs` 指定的 nodeId 作为“最终输出”，或
  - 自己扫描 `nodes` 里 type 为 `MaterialOutput` / `CompositeOutput` 的节点作为输出节点。

---

## 3. Node 与 Connection 的数据结构

### Node

```ts
interface Node {
  id: string;
  type: NodeType;               // 判别字段（discriminant）
  position: { x: number; y: number }; // 仅用于 Editor 布局，可忽略
  label?: string;               // UI 名称，可忽略

  // 运行时参数（非常重要）：
  // - 既包含“节点配置”（例如 RenderTexture.format）
  // - 也包含“inline inputs”（见第 6 节）
  params: Record<string, unknown>;

  // 动态端口（可选）：
  // - 主要用于 MathClosure 这类 closure 节点
  // - 生成器会把 editor 的 dynamicInputs 导出到这里
  inputs?: Port[];

  // 目前一般不依赖 node.outputs；静态输出口由 NodeDefinition 提供
  outputs?: Port[];

  // closure-style 节点的变量绑定（可选）
  inputBindings?: InputBinding[];
}
```

### Connection

```ts
interface Connection {
  id: string;
  from: { nodeId: string; portId: string }; // 输出端口
  to:   { nodeId: string; portId: string }; // 输入端口
}
```

**关键点：端口是用 `portId` 关联的。**

- 静态端口（绝大多数节点的输入/输出口）由 `NodeDefinition` 定义，端口 id 是稳定字符串（例如 `RenderPass.geometry` / `PBRMaterial.roughness`）。
- 动态端口（例如 `MathClosure` 新增的输入口）会生成一个随机/唯一 id（例如 `dynamic_123`），并出现在 `node.inputs` 里。

---

## 4. 最推荐的做法：直接复用 `@node-forge/dsl` 做解析与校验

如果你的 render server 也是 Node/TS 环境，最快路径是直接依赖 workspace 的 `@node-forge/dsl`：

```ts
import { parseDSL, validateSceneDSL, NODE_DEFINITIONS } from '@node-forge/dsl';

const { data: scene, error } = parseDSL(jsonString);
if (!scene) throw new Error(error ?? 'Invalid DSL');

const validation = validateSceneDSL(scene);
if (!validation.valid) {
  // validation.errors / validation.warnings
  throw new Error('SceneDSL failed validation');
}

// scene: SceneDSL
// NODE_DEFINITIONS: 每种 NodeType 的静态端口 & 默认参数
```

> 注意：当前 `validateSceneDSL` 主要做“schema + nodeId/connectionId/存在性”检查；**端口类型兼容**尚未实现（代码里有 TODO）。渲染端通常需要更严格的校验（见第 9 节）。

---

## 5. NodeDefinition：渲染端的“静态真相来源”

渲染端想正确执行节点，通常需要知道：

- 每个节点有哪些输入口/输出口（port ids、类型）
- 节点默认参数（defaultParams）
- 端口默认值（Port.default）与数值范围（Port.range）

这些在 [packages/dsl/src/nodes.ts](packages/dsl/src/nodes.ts) 的 `NODE_DEFINITIONS` 里。

```ts
interface NodeDefinition {
  type: NodeType;
  label: string;
  category: string;
  description: string;
  inputs: Port[];
  outputs: Port[];
  defaultParams: Record<string, unknown>;
}
```

渲染端通常会做一次“归一化”：

- `effectiveParams = { ...definition.defaultParams, ...node.params }`
- `effectiveInputs = definition.inputs + (node.inputs ?? [])`（用于支持动态输入口）

---

## 6. Inline inputs（常量输入）如何解析

Editor 里的约定（见 [packages/editor/docs/inline-inputs.md](packages/editor/docs/inline-inputs.md)）：

- inline 值写入 `node.data.params[portId]`
- 如果某个输入口有入边（已连接），inline 控件应隐藏/禁用（即“连线优先”）

因此渲染端解析某个节点输入口时，建议用如下优先级：

1. **如果 `connections` 里存在 `to.nodeId === node.id && to.portId === portId` 的连线**：
   - 输入来自上游节点的 `from.portId`
2. 否则，如果 `node.params` 里有同名键（`portId`）存在：
   - 输入来自 inline 常量（例如 number / vector / color）
3. 否则，如果 `Port.default` 存在：
   - 使用端口默认值
4. 否则：
   - 该输入缺失（你可以报错或按节点语义给 fallback）

> 这套规则对 `float/int/vector/color/...` 等 primitive 特别关键。

---

## 7. Input Bindings（闭包节点）如何解析

Input Bindings 的设计详见 [packages/dsl/docs/input-bindings.md](packages/dsl/docs/input-bindings.md)。渲染端需要关注的是：

```ts
interface InputBinding {
  portId: string;      // 对应输入口（通常是动态口）
  label: string;       // UI 显示名
  variableName: string;// 代码中引用的变量名（通常与 label 同步）
  type: PortType;
  sourceBinding?: {
    nodeId: string;
    outputPortId: string;
    outputLabel: string;
  };
}
```

### 在 `MathClosure` 上的典型用法

- `MathClosure` 的代码字符串在 `node.params.source`（见 [packages/dsl/src/nodes.ts](packages/dsl/src/nodes.ts)）。
- 节点的动态输入口在 `node.inputs`。
- 变量名映射在 `node.inputBindings`。

渲染端执行闭包时，建议按 `inputBindings` 构建运行时环境：

伪代码：

```ts
// 1) 解析并归一化
const def = NODE_DEFINITIONS[node.type];
const params = { ...def.defaultParams, ...node.params };
const dynamicInputs = node.inputs ?? [];
const bindings = node.inputBindings ?? [];

// 2) 解析每个动态输入口的值
const env: Record<string, unknown> = {};
for (const inputPort of dynamicInputs) {
  const b = bindings.find((x) => x.portId === inputPort.id);
  const varName = b?.variableName ?? inputPort.name; // 没 binding 时的兜底

  // value = resolveInputValue(node.id, inputPort.id)
  env[varName] = resolveInputValue(node.id, inputPort.id);
}

// 3) 执行 params.source
// 注意：source 是用户输入代码，服务端执行必须做沙箱隔离
const output = runClosure(params.source, env);

// 4) 写回该节点的输出口值
nodeOutputs['output'] = output;
```

> `sourceBinding` 字段更多用于溯源/调试；执行时你通常只需要 `portId -> variableName`。

---

## 8. 构建计算图（拓扑排序）

渲染端一般需要：

1. 构建 `nodeId -> Node` 的映射
2. 基于 `connections` 建立有向图（from.nodeId → to.nodeId）
3. 拓扑排序得到执行顺序
4. 按顺序执行节点，缓存每个节点的输出口结果

简化伪代码：

```ts
const nodesById = new Map(scene.nodes.map(n => [n.id, n]));

const outgoing = new Map<string, Connection[]>();
const incoming = new Map<string, Connection[]>();
for (const c of scene.connections) {
  (outgoing.get(c.from.nodeId) ?? outgoing.set(c.from.nodeId, []).get(c.from.nodeId)!).push(c);
  (incoming.get(c.to.nodeId) ?? incoming.set(c.to.nodeId, []).get(c.to.nodeId)!).push(c);
}

// topologicalSort(...) 需要检测环；有环时应该报错
const order = topologicalSort(scene.nodes, scene.connections);

const computedOutputs = new Map<string, Record<string, unknown>>();

for (const nodeId of order) {
  const node = nodesById.get(nodeId)!;
  const def = NODE_DEFINITIONS[node.type];
  const params = { ...def.defaultParams, ...node.params };

  // resolveInputValue(nodeId, portId) 根据 incoming + params + defaults
  const outputs = executeNode(node, def, params, resolveInputValue);
  computedOutputs.set(nodeId, outputs);
}
```

`resolveInputValue` 的关键就是第 6 节的优先级。

---

## 9. 建议在渲染端补充的校验（强烈建议）

当前 `validateSceneDSL` 有意保持轻量。渲染端为了稳定性，建议额外做：

- **端口存在性校验**：
  - `conn.from.portId` 必须存在于 source 节点定义的 outputs（或 node.outputs）
  - `conn.to.portId` 必须存在于 target 节点定义的 inputs + node.inputs
- **类型兼容校验**：
  - `sourcePort.type` 与 `targetPort.type` 是否一致或可隐式转换（例如 `int -> float`）
- **图环检测**：
  - 避免执行时死循环
- **必填输入校验**：
  - 例如 `RenderPass` 的 `geometry/material/target`（dsl 的 validator 目前仅做 warning）

---

## 10. 一个最小 DSL 示例（便于你写解析器）

下面示例直接使用 Editor 导出的一个最小文件（包含 `outputs.composite`）：

- `Rect2DGeometry` 产出 geometry
- `RenderTexture` 作为 render target
- `RenderPass` 把 geometry 渲到 target
- `CompositeOutput` 作为最终输出（`outputs.composite` 指向它）

```json
{
  "version": "1.0",
  "metadata": {
    "name": "My Scene",
    "created": "2026-01-05T15:43:15.799Z",
    "modified": "2026-01-05T15:43:15.799Z"
  },
  "nodes": [
    {
      "id": "node_1",
      "type": "Rect2DGeometry",
      "position": {
        "x": 740,
        "y": 160
      },
      "label": "2D Rect",
      "params": {
        "width": 100
      }
    },
    {
      "id": "node_2",
      "type": "RenderPass",
      "position": {
        "x": 1140,
        "y": 160
      },
      "label": "Render Pass",
      "params": {
        "name": "Render Pass"
      }
    },
    {
      "id": "node_3",
      "type": "Attribute",
      "position": {
        "x": 940,
        "y": 160
      },
      "label": "Attribute",
      "params": {
        "name": "uv",
        "glslType": "vec2"
      }
    },
    {
      "id": "node_4",
      "type": "RenderTexture",
      "position": {
        "x": 940,
        "y": 300
      },
      "label": "Render Texture",
      "params": {
        "width": 1024,
        "height": 1024,
        "format": "rgba8unorm"
      }
    },
    {
      "id": "node_6",
      "type": "CompositeOutput",
      "position": {
        "x": 1340,
        "y": 160
      },
      "label": "Composite Output",
      "params": {}
    }
  ],
  "connections": [
    {
      "id": "edge_1",
      "from": {
        "nodeId": "node_1",
        "portId": "geometry"
      },
      "to": {
        "nodeId": "node_2",
        "portId": "geometry"
      }
    },
    {
      "id": "edge_2",
      "from": {
        "nodeId": "node_1",
        "portId": "geometry"
      },
      "to": {
        "nodeId": "node_3",
        "portId": "geometry"
      }
    },
    {
      "id": "edge_3",
      "from": {
        "nodeId": "node_3",
        "portId": "value"
      },
      "to": {
        "nodeId": "node_2",
        "portId": "material"
      }
    },
    {
      "id": "edge_4",
      "from": {
        "nodeId": "node_4",
        "portId": "texture"
      },
      "to": {
        "nodeId": "node_2",
        "portId": "target"
      }
    },
    {
      "id": "edge_5",
      "from": {
        "nodeId": "node_2",
        "portId": "pass"
      },
      "to": {
        "nodeId": "node_6",
        "portId": "image"
      }
    }
  ],
  "outputs": {
    "composite": "node_6"
  }
}
```

说明：

- 像 `Rect2DGeometry` 这类节点的 `params` 可能只覆盖部分字段（例如这里只有 `width`），渲染端应按第 5 节规则与 `defaultParams` 合并得到最终参数。
- 该例里 `outputs.composite` 明确指定了最终输出节点；如果缺失，你可以回退到扫描 `CompositeOutput` / `MaterialOutput`。

---

## 11. 安全提示（如果你要执行用户代码）

`MathClosure.params.source` / `ShaderMaterial` 等节点包含用户编辑的代码文本。

- 如果 render server 在服务端执行：**不要直接 `eval`**。
- 建议：
  - 运行在沙箱（例如 isolate / wasm / 自研解释器）
  - 限制可访问 API、限制 CPU 时间与内存

---

## 12. 你大概率会用到的“入口文件”

- Schema & Types：
  - [packages/dsl/src/schema.ts](packages/dsl/src/schema.ts)
- 节点注册表（端口与默认参数）：
  - [packages/dsl/src/nodes.ts](packages/dsl/src/nodes.ts)
- 解析/生成/校验工具：
  - [packages/dsl/src/validator.ts](packages/dsl/src/validator.ts)
- Input Bindings 机制说明：
  - [packages/dsl/docs/input-bindings.md](packages/dsl/docs/input-bindings.md)

如果你希望我再补一个“渲染端最小可运行的 TS 解析器骨架”（包含拓扑排序 + 端口解析函数），我也可以直接在 `packages/server` 或新目录里给你加一个示例实现。