# Node Forge：接收 DSL 更新的 WebSocket Server 实现说明

本文档面向“实现/维护 server 的同学”。目标是明确：Editor 如何推送 DSL、Server 需要做什么、消息格式是什么、以及你可以在哪些位置扩展（持久化 / 转发给 renderer）。

> 代码参考：
> - 协议定义：`packages/server/src/protocol.ts`
> - WebSocket 服务端：`packages/server/src/server.ts`
> - Editor 客户端：`packages/editor/src/services/websocket.ts` + `packages/editor/src/hooks/useWebSocket.ts`
> - DSL 校验器：`packages/dsl/src/validator.ts`（`validateSceneDSL`）

---

## 1. 总体职责

“DSL 更新接收服务器”（以下简称 Server）当前在工程里的角色是一个 **WebSocket Hub**：

- 接收来自 Editor 的 `scene_update`（payload 为 `SceneDSL`）
- 用 `validateSceneDSL(payload)` 校验 DSL
- 校验通过后：
  - 更新 server 侧的 `currentScene`（内存态）
  - 将该更新广播给其它已连接客户端（包括可能的 renderer / 另一个 editor）
- 支持新连接/掉线重连时获取当前场景：
  - client 连接后 server 主动推一次当前场景（如果存在）
  - 或 client 主动发 `scene_request` 请求当前场景

这套设计允许你把 renderer 当做“另一个 WS client”：它只需要连上 WS 并监听 `scene_update` 即可拿到最新 DSL。

---

## 2. WebSocket 连接信息

### 2.1 默认地址

Editor 端默认连接：

- `ws://<window.location.hostname>:8080`

见 `packages/editor/src/services/websocket.ts` 中 `getWebSocketClient()`。

### 2.2 Server 启动参数

Server 入口：`packages/server/src/index.ts`

- `PORT`：默认 `8080`
- `HOST`：默认 `0.0.0.0`

启动后会输出：

- `WebSocket endpoint: ws://<HOST>:<PORT>`

---

## 3. 消息协议（WSMessage）

协议在 `packages/server/src/protocol.ts`。

所有消息都有公共字段：

```ts
interface BaseMessage {
  type: string;
  timestamp: number;
  requestId?: string;
}
```

当前约定的 `type`：

- `scene_update`（C->S, S->C）
- `scene_request`（C->S）
- `render_request`（C->S）
- `render_result`（S->C）
- `error`（S->C）
- `ping`（C->S）
- `pong`（S->C）

> 注意：这里的 `ping/pong` 是“应用层消息”。底层 WebSocket 也有自己的 ping frame（`ws` 库的 `ws.ping()` / `pong` 事件）。当前 server 同时做了两套保活：
> - **底层 heartbeat**：server 周期性 `ws.ping()`，靠 `pong` 事件判断连接是否存活
> - **应用层 ping**：Editor 每 25s 发送 `{type:'ping'}`，server 返回 `{type:'pong'}`

### 3.1 scene_update

client -> server：

```json
{
  "type": "scene_update",
  "timestamp": 1730000000000,
  "payload": { "version": "1.0", "metadata": { "name": "Live Scene" }, "nodes": [], "connections": [], "outputs": {} }
}
```

server -> client（广播给其它 client）：

- 同样是 `type: "scene_update"`
- `payload` 为最新的 `SceneDSL`

### 3.2 scene_request

client -> server：

```json
{ "type": "scene_request", "timestamp": 1730000000000 }
```

server 行为：如果 `currentScene` 存在，则回一条 `scene_update` 给请求方。

### 3.3 error

server -> client：

```json
{
  "type": "error",
  "timestamp": 1730000000000,
  "payload": {
    "code": "VALIDATION_ERROR",
    "message": "Invalid scene DSL",
    "details": [
      { "type": "error", "code": "SCHEMA_ERROR", "message": "nodes.0.id: Required" }
    ]
  }
}
```

目前常见的错误码：

- `PARSE_ERROR`：JSON 解析失败
- `VALIDATION_ERROR`：`validateSceneDSL(payload)` 不通过

---

## 4. DSL 校验要求（validateSceneDSL）

Server 收到 `scene_update` 后，必须做校验：

- `validateSceneDSL(sceneMessage.payload)`

校验器位置：`packages/dsl/src/validator.ts`

校验覆盖：

- Zod schema（结构字段、类型）
- nodeId / connectionId 重复
- connection source/target nodeId 存在性
- 一些 warning（例如没有 Output node、RenderPass 缺必需输入）

> 重要：目前端口类型兼容/端口存在性等更深层校验仍有 TODO；如果你把 server 当作“权威入口”，可以在 server 侧追加更严格的语义校验。

---

## 5. Server 端应有的核心行为（参考现有实现）

现有 `NodeForgeServer`（`packages/server/src/server.ts`）已经实现了以下行为，可以作为你实现的“验收标准”：

1. **连接管理**
   - 每个连接分配 `clientId`
   - 维护 `clients: Map<string, ClientInfo>`

2. **场景状态**
   - `currentScene: SceneDSL | null` 保存在内存

3. **接收 scene_update**
   - `parseMessage()` 解析 JSON
   - `validateSceneDSL()` 校验
   - 更新 `currentScene`
   - `broadcast(scene_update, exclude=[sender])`

4. **新连接同步**
   - client connect 时，如果 `currentScene` 存在，server 主动 send 一条 `scene_update`
   - client 也可用 `scene_request` 获取

5. **保活与超时断开（底层 heartbeat）**
   - server 定期 `ws.ping()`
   - 如果 client 没有回应 pong，则 `terminate()` 并移除

---

## 6. Editor 推送频率与去抖

理解 Editor 的发送模型有助于你决定 server 的吞吐/存储策略：

- `useWebSocket()`（`packages/editor/src/hooks/useWebSocket.ts`）在 `autoSync` 开启且 WS 已连接时，会对 `nodes/edges` 变化做 **300ms debounce**，然后调用 `sendSceneUpdate(nodes, edges)`
- `sendSceneUpdate()`（`packages/editor/src/services/websocket.ts`）会：
  - 将 editor 的 nodes/edges 转换为 DSL 输入
  - 调用 `generateDSL()`（`@node-forge/dsl`）生成 `SceneDSL`
  - 发送 `type: 'scene_update'`

因此 server 端通常会看到一个连续的更新流（拖拽、连线、修改参数都会触发）。

建议：

- **不要**在 server 侧对每次 update 都做昂贵的同步 IO（例如写数据库）
- 如果需要持久化，优先做 server 侧 debounce/batch（例如 500ms~2s 写一次）或按版本号/时间戳节流

---

## 7. Renderer/别的客户端如何接入

如果你要实现 renderer：

- 连接 WS：`ws://<host>:8080`
- 监听消息：
  - `scene_update`：获取最新 `SceneDSL`
  - （可选）`render_request`：如果你把 server 做成“转发器”，renderer 可通过 server 收到 render 请求

解析 DSL 的细节请直接参考现有文档：

- `packages/dsl/docs/render-server-dsl-parsing.md`

---

## 8. 扩展点（你很可能要改的地方）

### 8.1 持久化 currentScene

现状：`currentScene` 仅存内存，server 重启会丢。

常见方案：

- 写到本地文件（例如 `./data/currentScene.json`）
- 写到 KV/DB（Redis/Postgres 等）

推荐落点：

- 在 `scene_update` handler 校验通过后
- 但要做节流/去抖（避免每次拖拽都写盘）

### 8.2 多房间 / 多场景

现状：所有 client 共享一个 `currentScene`。

如果需要多房间：

- 让 client 在连接 url 带 `?room=<id>` 或先发 `join_room` 消息
- server 侧改为 `Map<roomId, SceneDSL>` 与 `Map<roomId, Set<clientId>>`

### 8.3 安全与鉴权

现状：无鉴权。

如需最小鉴权：

- client 连接时带 token（query/header）
- server 在 `connection` 事件上校验，通过才 accept

### 8.4 render_request / render_result

`render_request` 在 server 侧目前只是 TODO。

两种常见集成方式：

1) server 直接调用 renderer（同进程 / 子进程 / HTTP）并回 `render_result`
2) server 仅做转发：
   - Editor 发 `render_request` 到 server
   - server 转发给 renderer client
   - renderer 回 `render_result` 给 server
   - server 再转发给请求方（按 `requestId` 关联）

协议字段已经预留了 `requestId`，建议对 `render_request` 强制带 `requestId`。

---

## 9. 本地开发与运行

在仓库根目录：

```bash
pnpm install

# 启动 server（ws://localhost:8080）
pnpm -C packages/server dev

# 启动 editor（默认会连 ws://<host>:8080）
pnpm -C packages/editor dev
```

自定义端口：

```bash
PORT=9000 HOST=0.0.0.0 pnpm -C packages/server dev
```

如果你改了 server 端口，Editor 端需要在 `getWebSocketClient(url)` 传入自定义 url，或者调整默认拼接逻辑（见 `packages/editor/src/services/websocket.ts`）。
