# Freeze `1787822441687` Android / Node Forge 对照报告

## 结论

这次整帧还没有完全对齐。

- 整帧 RGB **Max AE：`0.9806345701`**
- Android HWC 解算峰值：`1.5126738548`
- Node Forge 峰值：`1.5`
- 峰值绝对差：`0.0126738548`
- 在 Node Forge 峰值坐标 `(419, 2428)`：Android `1.4907991886`，绝对误差 `0.0092008114`

全局 Max AE 出现在 PTT 字图的硬边缘；更重要的剩余结构性误差来自 Node Forge 当前拓扑中的多级 `BloomNode` 与 Android 仍在使用的旧解析式光效管线不一致。HWC input 解算和 Intelligent Light 的 FP16 中间材质不是这次大误差的来源。

## Case

- Case ID：`freeze-1787822441687-1`
- Scene time：`23.824951 s`
- Freeze topology fingerprint：`4ddae9edee77ee4bc67de06ca11bdcde502bbcade4f2960cf291779a8488301d`
- 当前存档 pipeline fingerprint：`92004da841ef4e345f3b7ddbd1634e5749aa7d21b34a6eb233f38a5e3411c72f`（Render Server 会警告 fingerprint 不同，但继续做 semantic replay）
- Replay JSON SHA-256：`807a80ef4915e02fb22c36260b52c1d212be1689fc9132b5398a74cdeb871d5c`

## 方法

1. Render Server 用 `doubao-voice-interaction.nforge` 构建当前拓扑，并用 `--freeze-case freeze-1787822441687-1` 离线输出 `1260 × 2800` EXR。
2. Android 用同一份 replay JSON 冷启动播放该帧。
3. `hwc_dump_tool` 抓 app input layer：`1280 × 2816 RGBA_8888`、非 UBWC。
4. 根据 SurfaceFlinger 的 HDR/SDR ratio `3.0`，在 Mac 本机将 HWC input 的 Display-P3/sRGB-transfer 数据反归一化为 linear EXR，并中心裁出 `1260 × 2800` 内容区。
5. Android EXR 与 Node Forge EXR 在同一 linear RGB 域逐像素比较，不做图像配准或亮度拟合。

抓帧时 SurfaceFlinger 状态：

- App layer dataspace：`0x188a0000`
- current / desired HDR-SDR ratio：`3.0 / 3.0`
- Display color mode：`DISPLAY_P3`

## 整帧指标

| 指标 | 结果 |
|---|---:|
| RGB Max AE | `0.9806345701` |
| R Max AE | `0.9806345701` |
| G Max AE | `0.9565441608` |
| B Max AE | `0.9064763188` |
| RGB AE P95 | `0.0098954812` |
| RGB AE P99 | `0.0632926598` |
| RGB AE P99.9 | `0.3380709291` |

Max AE 样本：

- 坐标：`(475, 2435)`，R
- Android：`0.3767873049`
- Node Forge：`1.357421875`
- 绝对误差：`0.9806345701`

它位于 PTT PNG 的高对比度文字边缘。两端纹理峰值位置相差约 1 px；这是覆盖率/采样边缘误差，不是 HDR 峰值幅度错误，也不是整张画面的全局平移。

## 分阶段归因

### 1. Intelligent Light FP16 intermediate 已对齐

直接比较双方 `84 × 84 RGBA16F` 中间材质：

| 指标 | 结果 |
|---|---:|
| RGBA Max AE | `0.0009765625` |
| R Max AE | `0.0000610352` |
| G Max AE | `0.0002899170` |
| B Max AE | `0.0009765625` |
| A Max AE | `0.0009765625` |

本轮修正了 Android replay 颜色合同：freeze 的 color 字段必须作为 sRGB-tagged color 进入 Android，再由材质输入层转换为 Linear Extended sRGB。此前直接把 payload 标记成 Linear Extended sRGB，会让该帧蓝通道峰值从 Node Forge 的约 `0.3049` 错升到 Android 的约 `0.8726`。

### 2. 背景区域已接近

顶部背景区域（`y < 1400`）Max AE 为 `0.0092245638`。这说明 HWC input + headroom 3.0 的本机解算、Display-P3 到比较域的转换以及背景 linear darken 路径工作正常。

### 3. 剩余光效误差来自拓扑不同

排除 PTT 区域后，最大误差仍为 `0.9064763188`，坐标 `(218, 2700)` 的 B 通道：

- Android：`0.1277033538`
- Node Forge：`1.0341796875`

Intelligent Light 输入材质已经对齐，但其下游不同：

- Node Forge 当前存档拓扑先经过独立的多级 `BloomNode_e44f1972_54`（extract、mip downsample、横/纵 blur、upsample/add），最终 Light Effect shader 直接返回 bloom texture。
- Android `voice_visualizer.agsl` 仍在一个 draw call 内用 SDF envelope 和解析式 `glow` 近似旧光效。

所以中心区域可以接近，而大范围 bloom 的空间能量分布会明显不同。这个问题不能再通过 headroom、blend state、颜色空间或 Intelligent Light 增益修正；要继续降低整帧 Max AE，需要让 Android 下游也实现当前 Node Forge 的 BloomNode 拓扑。

## 产物

- `node-forge.exr`：Node Forge 离线参考
- `android-hwc-input-linear.exr`：Android HWC input 本机解算 EXR
- `android-hwc-input-linear.rgba16f`：同一 Android linear 数据的 NF16
- `android-hwc-input_1280x2816_RGBA_8888_frame0.raw`：原始 HWC input layer
- `node-forge-intelligent-light-84.exr` / `android-intelligent-light-84.rgba16f`：Intelligent Light 阶段对照
- `comparison-metrics.json`：完整数值指标
- `comparison-contact-sheet.png`：整帧 Node / Android / diff
- `bottom-contact-sheet.png`：主要差异区域
- `effect-stage-contact-sheet.png`：下游光效阶段可视化

关键文件 SHA-256：

- Node Forge EXR：`331e058c7a7b4843774ac0535ee32d547b73670c972b1c3aff38f5c10c4d1cb6`
- Android HWC raw：`f422e7dbcf3284a4b071cfb2660a371850319725d83b0dde02c538b6f28c537a`
- Android reconstructed EXR：`f8b72828f9dce0dd0d521c349a1b58a80ea5a82b2a94152296373adecfc8f9ac`
