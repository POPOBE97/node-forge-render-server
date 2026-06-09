# 实现计划：State Machine Sidebar Panel

## 概述

在调试侧边栏底部新增 State Machine 折叠树面板，实时展示动画状态机运行状态。实现分为：数据类型定义、快照提取逻辑、格式化工具函数、UI 渲染函数、以及集成到现有侧边栏。

## 任务

- [x] 1. 定义核心数据类型与格式化工具函数
  - [x] 1.1 创建 `src/ui/state_machine_panel.rs` 模块，定义 `StateMachineSnapshot` 和 `StateInfo` 结构体
    - `StateMachineSnapshot` 包含：name, id, current_state_id, current_state_name, finished, scene_time_secs, active_transition_id, transition_blend, transition_source_name, transition_target_name, states, state_local_times, override_values
    - `StateInfo` 包含：id, name, state_type, is_current
    - 在 `src/ui/mod.rs` 中注册新模块
    - _需求: 1.1, 1.2_

  - [x] 1.2 实现 `format_f64_2dp` 和 `format_json_value_2dp` 格式化函数
    - `format_f64_2dp`: 有限值格式化为两位小数，NaN 返回 `"NaN"`，无穷大返回 `"Inf"`
    - `format_json_value_2dp`: 数值类型两位小数，数组逐元素格式化为 `[a, b, ...]`，布尔/字符串直接输出
    - _需求: 2.1, 2.2, 2.3, 2.4, 2.5_

  - [ ]* 1.3 为 `format_f64_2dp` 编写属性测试
    - **Property 1: format_f64_2dp 对有限值始终返回两位小数**
    - 对任意有限 f64 值，返回值匹配正则 `^-?\d+\.\d{2}$`；NaN 返回 `"NaN"`；Inf 返回 `"Inf"`
    - **验证: 需求 2.1, 2.2, 2.3**

  - [ ]* 1.4 为 `format_json_value_2dp` 编写属性测试
    - **Property 4: JSON 值格式化保持类型语义**
    - 数值类型格式化为两位小数；数值数组逐元素格式化；布尔/字符串直接输出原始值
    - **验证: 需求 2.4, 2.5**

- [x] 2. 实现快照提取逻辑
  - [x] 2.1 实现 `snapshot_from_session` 函数
    - 从 `AnimationSession` 提取 `StateMachineRuntime` 数据构建 `StateMachineSnapshot`
    - 遍历 `definition().states` 构建 `states` 列表，标记 `is_current`
    - 查找当前状态名称，解析活跃转场的源/目标状态名称
    - _需求: 1.1, 1.2, 1.3_

  - [ ]* 2.2 为快照 states 数量编写属性测试
    - **Property 2: 快照 states 数量等于定义中的 states 数量**
    - **验证: 需求 1.2**

  - [ ]* 2.3 为 is_current 唯一性编写属性测试
    - **Property 3: 恰好有一个状态标记为 is_current**
    - **验证: 需求 1.3**

  - [ ]* 2.4 为转场一致性编写属性测试
    - **Property 5: 无转场 id 则无混合因子**
    - 若 `active_transition_id` 为 None，则 `transition_blend` 必须为 None
    - **验证: 需求 4.4**

- [x] 3. 检查点 - 确保所有测试通过
  - 确保所有测试通过，如有疑问请询问用户。

- [x] 4. 实现 UI 渲染函数
  - [x] 4.1 实现 `show_state_machine_section` 主渲染函数及 `label_value` 辅助函数
    - 使用 `egui::CollapsingHeader` 渲染四个子树：Status、Transition、States、Values
    - `label_value` 使用水平布局，左侧标签右侧等宽值
    - _需求: 3.1, 4.1, 4.2, 5.1, 5.4, 6.1, 6.2, 7.3_

  - [x] 4.2 实现 Status 子树
    - 默认展开，显示 Name、Current State、Scene Time（两位小数）、Finished
    - _需求: 3.1, 3.2, 3.3_

  - [x] 4.3 实现 Transition 子树
    - 仅在 `active_transition_id` 存在时渲染，默认展开
    - 显示 From（源状态名）、To（目标状态名）、Blend（两位小数）
    - _需求: 4.1, 4.2, 4.3_

  - [x] 4.4 实现 States 子树
    - 默认折叠，列出所有状态及本地时间（两位小数）
    - 当前活跃状态名称旁显示 `●` 标记
    - _需求: 5.1, 5.2, 5.3, 5.4_

  - [x] 4.5 实现 Values 子树
    - 默认展开，列出所有 override 键值对
    - 空列表时显示 `"(no active overrides)"` 占位文本
    - _需求: 6.1, 6.2, 6.3_

- [x] 5. 集成到调试侧边栏
  - [x] 5.1 修改 `show_in_rect` 函数签名，新增 `sm_snapshot: Option<&StateMachineSnapshot>` 参数
    - 在 Resource Tree section 之后、ScrollArea 内部添加 State Machine 面板
    - 使用 `section_divider` 分隔，`with_sidebar_content_padding` 包裹
    - 仅在 `sm_snapshot` 为 `Some` 时渲染
    - _需求: 1.4, 7.1, 7.2_

  - [x] 5.2 修改 `src/app/mod.rs` 中的调用方，构建快照并传递给 `show_in_rect`
    - 从 `AnimationSession` 调用 `snapshot_from_session` 构建快照
    - 补充来自 `AnimationStep` 的动态数据（state_local_times, transition_blend, override_values）
    - 更新 `show_in_rect` 调用处传入 `sm_snapshot`
    - _需求: 1.1, 1.4_

- [x] 6. 最终检查点 - 确保所有测试通过
  - 确保所有测试通过，如有疑问请询问用户。

## 备注

- 标记 `*` 的任务为可选任务，可跳过以加速 MVP 开发
- 每个任务引用了具体需求编号以确保可追溯性
- 检查点确保增量验证
- 属性测试验证通用正确性属性，单元测试验证具体示例和边界情况
