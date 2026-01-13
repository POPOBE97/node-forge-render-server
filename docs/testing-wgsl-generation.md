# WGSL 生成测试（DSL JSON → WGSL）

本仓库的 WGSL 生成测试用于覆盖「输入 SceneDSL JSON → 生成每个 RenderPass 的 WGSL（vertex/fragment/可选 compute）」，并在 `cargo test` 中做两类校验：

- **Golden 对比**：生成结果必须与预期输出文件一致（防止节点扩展时出现回归）。
- **语法校验**：用 `naga` 解析 `module.wgsl`，确保 WGSL 至少是语法有效的（不依赖 GPU）。

---

## 1. 用例目录结构（支持一组测试里包含多个输入 case）

当前 `wgsl_generation` 测试会扫描目录 `tests/cases/wgsl_generation/` 下所有 `*.json`：

- 每个 `*.json` 都是一组输入 case，例如：`simple_1.json`、`blend_2.json`。

并且会为每个 RenderPass 生成/维护对应的 golden 文件（按 case + pass 的 nodeId 命名）：

- `<case>.<pass_id>.vertex.wgsl`
- `<case>.<pass_id>.fragment.wgsl`
- `<case>.<pass_id>.module.wgsl`

示例：

- `tests/cases/wgsl_generation/simple_1.json`
- `tests/cases/wgsl_generation/simple_1.node_2.vertex.wgsl`
- `tests/cases/wgsl_generation/simple_1.node_2.fragment.wgsl`
- `tests/cases/wgsl_generation/simple_1.node_2.module.wgsl`

> 说明：当前实现以 RenderPass 的 nodeId 作为 pass_id，因此 golden 文件名直接用 nodeId。

---

## 2. 运行测试

在仓库根目录：

```bash
cargo test
```

---

## 2.1 用命令行直接做一次 headless 渲染（给脚本用）

当你在写测试脚本时，如果手里已经有一个 SceneDSL 的 JSON 文件，并且这个 scene 的 RenderTarget 是 `File`，可以直接通过命令行一次性 headless 渲染并输出到指定目录。

要求：

- 必须带 `--headless`
- 必须带 `--dsl-json <scene.json>`（只支持文件路径）
- 必须带 `--outputdir <dir>`
- 输出文件名使用 RenderTarget(File) 节点的 `fileName`（会先应用 scheme 默认值）；`--outputdir` 会覆盖 scene 里的 `directory`

示例：

```bash
cargo run -q -- \
  --headless \
  --outputdir ./tmp/out \
  --dsl-json ./tests/cases/wgsl_generation/simple_1.json
```

成功时进程会输出类似：

```text
[headless] saved: <outputdir>/<fileName>
```

失败时会返回非零退出码并打印错误信息。

---

## 3. 生成/更新 golden 输出（第一次建用例、或 WGSL 预期变更时）

当你新增节点、调整 WGSL 输出格式，或者第一次创建/新增某个 `*.json` case 时：

```bash
UPDATE_GOLDENS=1 cargo test
```

行为：

- 测试会遍历 `tests/cases/wgsl_generation/*.json`
- 对每个输入生成所有可达的 RenderPass 的 WGSL
- 把输出写入对应的 `*.wgsl` golden 文件（见上面的命名规则）

之后再跑一次普通测试确认对比稳定：

```bash
cargo test
```

---

## 4. 如何新增一个测试 case（推荐流程）

1) 在 `tests/cases/wgsl_generation/` 下新增一个输入文件：

- `<your_case>.json`

2)（可选但推荐）先跑一次生成/更新 golden：

```bash
UPDATE_GOLDENS=1 cargo test
```

3) 再跑一次确认通过：

```bash
cargo test
```

---

## 5. 对比失败时怎么排查

- 先确认是否“确实应该变更输出”。如果是，直接用 `UPDATE_GOLDENS=1 cargo test` 更新 golden。
- 如果不应该变更：
  - 查看 diff 的 `*.wgsl` 文件，定位具体是哪个 pass_id 变了。
  - 如果是 WGSL 语法错误，测试会在 `naga` 解析阶段报错并打印该 pass 的 WGSL。

---

## 6. 相关代码位置

- 测试入口：`tests/wgsl_generation.rs`
- 生成 API：`src/renderer.rs`（`build_all_pass_wgsl_bundles_from_scene` / `build_pass_wgsl_bundle`）
