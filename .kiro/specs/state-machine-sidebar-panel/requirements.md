# 需求文档

## 简介

在左侧调试侧边栏底部（Resource Tree 下方）新增一个 "State Machine" 折叠树面板，实时展示当前动画状态机的运行状态，包括当前状态、转场信息、场景时间、各状态本地时间以及所有动画 override 值。所有浮点数统一格式化为两位小数。

## 术语表

- **StateMachineSnapshot**: 每帧从 AnimationSession 提取的状态机快照数据结构，用于 UI 渲染
- **AnimationSession**: 动画会话，管理状态机运行时和动画步进
- **StateMachineRuntime**: 状态机运行时引擎，维护当前状态、转场和定义
- **OverrideValue**: 动画系统产生的节点参数覆盖值（键值对）
- **DebugSidebar**: 左侧调试侧边栏 UI 组件（`src/ui/debug_sidebar.rs`）
- **format_f64_2dp**: 将 f64 值格式化为两位小数字符串的工具函数
- **StateInfo**: 单个状态的摘要信息（id、名称、类型、是否当前状态）
- **TransitionBlend**: 转场混合因子，范围 0.0 到 1.0

## 需求

### 需求 1：状态机快照数据提取

**用户故事：** 作为开发者，我希望每帧从 AnimationSession 中提取状态机快照数据，以便 UI 层能够安全地展示状态机运行状态而无需直接持有 runtime 引用。

#### 验收标准

1. WHEN AnimationSession 存在时，THE StateMachineSnapshot SHALL 包含状态机名称、id、当前状态 id 和当前状态名称
2. WHEN 快照从 AnimationSession 提取时，THE StateMachineSnapshot 的 states 列表 SHALL 包含定义中的所有状态，每个状态包含 id、name、state_type 和 is_current 字段
3. WHEN 快照构建完成时，THE StateMachineSnapshot 中恰好有一个状态的 is_current 字段 SHALL 为 true
4. WHEN AnimationSession 不存在时，THE DebugSidebar SHALL 不渲染 State Machine 面板

### 需求 2：浮点数格式化

**用户故事：** 作为开发者，我希望所有浮点数值统一格式化为两位小数，以便面板显示整洁、易读。

#### 验收标准

1. THE format_f64_2dp 函数 SHALL 将有限 f64 值格式化为恰好两位小数的字符串（如 `3.14`、`0.00`、`-1.50`）
2. WHEN 输入值为 NaN 时，THE format_f64_2dp 函数 SHALL 返回字符串 `"NaN"`
3. WHEN 输入值为无穷大时，THE format_f64_2dp 函数 SHALL 返回字符串 `"Inf"`
4. WHEN override 值为 JSON 数组时，THE 格式化函数 SHALL 将数组中每个数值元素分别格式化为两位小数并以 `[a, b, ...]` 格式输出
5. WHEN override 值为布尔或字符串类型时，THE 格式化函数 SHALL 直接输出原始值而不做小数格式化

### 需求 3：Status 子树展示

**用户故事：** 作为开发者，我希望在面板中看到状态机的基本运行状态，以便快速了解当前状态机处于什么状态。

#### 验收标准

1. THE Status 子树 SHALL 默认展开，显示状态机名称、当前状态名称、场景时间和是否已结束
2. WHEN 场景时间显示时，THE Status 子树 SHALL 使用两位小数格式化场景时间值
3. WHEN 状态机已到达 exit state 时，THE Status 子树的 Finished 字段 SHALL 显示 `"true"`

### 需求 4：Transition 子树展示

**用户故事：** 作为开发者，我希望在状态转场时看到转场详情，以便调试转场逻辑和混合效果。

#### 验收标准

1. WHEN 存在活跃转场时，THE Transition 子树 SHALL 显示转场源状态名称、目标状态名称和混合因子
2. WHEN 不存在活跃转场时，THE DebugSidebar SHALL 不渲染 Transition 子树
3. WHEN 转场混合因子显示时，THE Transition 子树 SHALL 使用两位小数格式化混合因子值
4. WHEN active_transition_id 为 None 时，THE StateMachineSnapshot 的 transition_blend SHALL 为 None

### 需求 5：States 子树展示

**用户故事：** 作为开发者，我希望看到所有状态的列表及其本地经过时间，以便了解各状态的运行情况。

#### 验收标准

1. THE States 子树 SHALL 列出所有状态，每个状态显示名称和本地经过时间
2. WHEN 某状态为当前活跃状态时，THE States 子树 SHALL 在该状态名称旁显示 `●` 标记
3. WHEN 状态本地时间显示时，THE States 子树 SHALL 使用两位小数格式化时间值
4. THE States 子树 SHALL 默认折叠

### 需求 6：Values 子树展示

**用户故事：** 作为开发者，我希望看到所有动画 override 值，以便实时监控动画参数的变化。

#### 验收标准

1. THE Values 子树 SHALL 默认展开，列出所有当前活跃的 override 键值对
2. WHEN override 值列表为空时，THE Values 子树 SHALL 显示 `"(no active overrides)"` 占位文本
3. WHEN override 值包含数值类型时，THE Values 子树 SHALL 以两位小数格式显示数值

### 需求 7：面板布局与集成

**用户故事：** 作为开发者，我希望 State Machine 面板位于侧边栏底部且与现有 UI 风格一致，以便获得统一的调试体验。

#### 验收标准

1. THE State Machine 面板 SHALL 位于 Resource Tree 区域下方，使用 section_divider 分隔
2. THE State Machine 面板 SHALL 使用 with_sidebar_content_padding 包裹内容，与现有侧边栏样式保持一致
3. THE show_state_machine_section 函数 SHALL 为纯展示函数，不产生任何副作用
