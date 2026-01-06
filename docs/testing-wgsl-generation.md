# WGSL 生成测试（DSL JSON → WGSL）

本仓库的 WGSL 生成测试用于覆盖「输入 SceneDSL JSON → 生成每个 RenderPass 的 WGSL（vertex/fragment/可选 compute）」，并在 `cargo test` 中做两类校验：

- **Golden 对比**：生成结果必须与预期输出文件一致（防止节点扩展时出现回归）。
- **语法校验**：用 `naga` 解析 `module.wgsl`，确保 WGSL 至少是语法有效的（不依赖 GPU）。

---

## 1. 用例目录结构（每个 test 一个 dedicated 文件夹）

每个测试用例放在自己的目录：

- `tests/cases/<case_name>/`

其中最少包含：

- `input.json`：输入的 SceneDSL JSON

并且会为每个 RenderPass 生成/维护对应的 golden 文件（按 pass 的 nodeId 命名）：

- `<pass_id>.vertex.wgsl`
- `<pass_id>.fragment.wgsl`
- `<pass_id>.module.wgsl`

示例：

- `tests/cases/wgsl_generation/input.json`
- `tests/cases/wgsl_generation/node_2.vertex.wgsl`
- `tests/cases/wgsl_generation/node_2.fragment.wgsl`
- `tests/cases/wgsl_generation/node_2.module.wgsl`

> 说明：当前实现以 RenderPass 的 nodeId 作为 pass_id，因此 golden 文件名直接用 nodeId。

---

## 2. 运行测试

在仓库根目录：

```bash
cargo test
```

---

## 3. 生成/更新 golden 输出（第一次建用例、或 WGSL 预期变更时）

当你新增节点、调整 WGSL 输出格式，或者第一次创建某个 case 时：

```bash
UPDATE_GOLDENS=1 cargo test
```

行为：

- 测试会读取 `tests/cases/<case_name>/input.json`
- 生成所有可达的 RenderPass 的 WGSL
- 把输出写入对应的 `*.wgsl` golden 文件

之后再跑一次普通测试确认对比稳定：

```bash
cargo test
```

---

## 4. 如何新增一个测试 case（推荐流程）

1) 新建目录：

- `tests/cases/<your_case_name>/`

2) 放入 `input.json`（SceneDSL JSON）。

3)（可选但推荐）先跑一次生成 golden：

```bash
UPDATE_GOLDENS=1 cargo test
```

4) 再跑一次确认通过：

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
