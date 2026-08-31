# Freeze `freeze-1787716149208-1` Android / Node Forge 对比报告

生成时间：2026-08-27（Asia/Shanghai）

## 结论

- `Android 1.14258 / Node Forge 1.94141` 不是 Android 材质管线产生的差异，而是用 stock PNG/gainmap 代替 App-owned FP16 读回后产生的 HDR 峰值损失。
- Android 的两级原始 FP16 材质结果都与 Node Forge 对齐：`84 × 84 Intelligent Light` 的 Max AE 为 `0.0048828125`，`1260 × 1260 VoiceVisualizer` 的 Max AE 为 `0.0107421875`。
- VoiceVisualizer 峰值在 Android 原始 FP16 中是 `1.869140625`，Node Forge 中是 `1.876953125`；Android 材质并没有把 HDR 峰值压到 `1.14`。
- 将 Android 同一材质的 raw FP16 与黑底 effect-only 截屏对比，Max AE 为 `0.7255859375`。最大误差点 raw RGBA 为 `[1.5625, 0.072265625, 1.869140625, 1.0]`，截屏重建后为 `[1.447265625, 0.0384521484375, 1.1435546875, 1.0]`。
- 该点 source alpha 为 `1.0`，目标是黑色；Node Forge 的 linear RGB add + SrcOver alpha draw-call 在这个点数学上必须保持 source RGB。因此这个 `0.7256` 损失不能归因于 blend state。
- SurfaceFlinger 显示 App layer 已是 extended Display-P3 dataspace，`current hdr/sdr ratio=3.0`。但当前命令明确记录 `fallback=helper-unavailable kind=gainmap-png`，所以对比用 EXR 是屏幕截图 gainmap 的反演，不是 App layer 的原始 FP16 数据。
- App 内部 `PixelCopy` 到 linear `RGBA_F16` 的 Window 读回数据被 HDR headroom `3.0` 归一化；三通道的 Node/Android 最小二乘比例为 `3.0078 / 3.0078 / 3.0034`。使用 Window 声明的 `3.0` 反归一化后，原峰值坐标 `(657,2797)` 变为 Android `1.9306640625`、Node Forge `1.94140625`，Max AE 只有 `0.0107421875`。
- 另一个真实 Android 问题被找到并修复：Gradient Blur 的 mip 金字塔使用 `ARGB_8888` 在非线性 8-bit 空间预缩放。背景源纹理 Max AE 只有 `0.00244140625`，经旧 mip 链后变为 `0.2877197265625`；改为 linear `RGBA_F16` mip 链后降到 `0.018798828125`。
- 修复背景 mip 后，Android 最终 Window FP16（按 headroom 3.0 反归一化）对 Node Forge 最终 EXR 的全帧 Max AE 是 `0.0341796875`，最大误差位于底部光效 `(704,2796)` 蓝通道：Android `1.7666015625`，Node Forge `1.80078125`。

阶段实验的 Max AE：

| 边界 | Max AE | 判断 |
|---|---:|---|
| Android / NF Intelligent Light `84 × 84` raw FP16 | `0.0048828125` | 对齐 |
| Android / NF VoiceVisualizer raw FP16 | `0.0107421875` | 对齐 |
| Android / NF 背景源纹理 | `0.00244140625` | 对齐 |
| Android / NF Background Blur（旧 `ARGB_8888` mip） | `0.2877197265625` | Android 中间精度/传递函数错误 |
| Android / NF Background Blur（新 linear `RGBA_F16` mip） | `0.018798828125` | 修复 |
| Android / NF 最终 Window FP16（headroom 反归一化） | `0.0341796875` | 最终 renderer 结果 |
| Android VoiceVisualizer raw FP16 / Android effect-only gainmap 截屏 | `0.7255859375` | 主要分歧首次出现 |
| Android effect-only gainmap 截屏 / NF `RenderPass_26` | `0.7333984375` | 与上一项一致 |
| Android 全帧 gainmap 截屏 / NF 最终 EXR | `0.798828125` | 包含背景合成后的旧指标 |

结论：就这一帧而言，Android 最终 renderer 的全帧 Max AE 是 `0.0341796875`，不是 `0.798828125`。旧数字主要是 stock gainmap screenshot 观测边界导致；同时实验确认并修复了背景 Gradient Blur 的 8-bit 非线性 mip 链。

## PNG 边界更正

初版产物曾把 linear sample 直接量化写入普通 16-bit PNG。普通查看器按 sRGB 显示该文件，因此画面会明显偏暗；该文件已撤回并替换为带 sRGB profile、执行过 linear → sRGB transfer 的 `android-private-srgb-rgba16.png`。

这不是 Android 渲染退化。将上一轮 Android linear EXR 与本轮 Android linear EXR直接比较：

| 指标 | 全帧 | 底部 436 行 |
|---|---:|---:|
| RGB MAE | 0.00000996 | 0.00006394 |
| RGB RMSE | 0.00025379 | 0.00064315 |
| RGB error > 0.05 | 0% | 0% |

两次实际捕获在数值上几乎相同；“看起来更差”来自错误的 PNG transfer boundary。

## 执行过程

Android replay：

```bash
adb shell am force-stop com.example.voice_interaction
adb shell am start \
  -n com.example.voice_interaction/.MainActivity \
  --es frame_replay doubao-voice-interaction-freeze-1787716149208-1
```

真机日志：

```text
FrameReplay: loaded case=freeze-1787716149208-1
VoicePerf: created 84x84 RGBA_FP16 linear-sRGB GPU intermediate static=true
VoicePerf: completed 84x84 RGBA_FP16 linear-sRGB GPU intermediate
```

Android source-profile 抓取：

```bash
node-forge-render-server --android-screencap-clipboard source
```

实际 capability：

```text
kind=gainmap-png
fallback=helper-unavailable
format=Rgba16Float
alpha=Straight
```

Node Forge 离线回放：

```bash
node-forge-render-server \
  --headless \
  --nforge ../node-forge-editor/examples/doubao-voice-interaction.nforge \
  --freeze-case freeze-1787716149208-1 \
  --render-to-file \
  --output node-forge.exr
```

## 旧 stock gainmap 截屏统计（不代表 renderer raw 输出）

| 指标 | 全帧 | 底部 436 行 | 底部 100 行 |
|---|---:|---:|---:|
| RGB MAE | 0.0139625 | 0.0044444 | 0.0116529 |
| RGB RMSE | 0.0263202 | 0.0186884 | 0.0378953 |
| 最大单通道绝对误差 | 0.798828 | 0.798828 | 0.798828 |
| 亮度 MAE | 0.0020743 | 0.0009973 | 0.0026636 |
| Android 平均亮度 | 0.146765 | 0.0169446 | 0.0396841 |
| Node Forge 平均亮度 | 0.146425 | 0.0163215 | 0.0381706 |
| Android / Node Forge 平均亮度 | 1.00232 | 1.03818 | 1.03965 |
| 任一 RGB 误差 > 0.05 的像素 | 13.1175% | 2.4656% | 10.7500% |

全帧绝对 RGB 误差百分位：`p50=0.00452`、`p95=0.05127`、`p99=0.07922`、`p99.9=0.20898`。

## HDR 范围

| 输出 | RGB 最小值 | RGB 最大值 | RGB sample > 1 |
|---|---:|---:|---:|
| Android gainmap 重建 EXR | -0.07214 | 1.46289 | 221,697 |
| Node Forge EXR | 0.00000 | 1.94141 | 3,928 |

Android 的负 RGB 来自 Display-P3 / gainmap 转到 extended linear sRGB 后保留的广色域分量。Node Forge 的最大值集中在底部光效峰值；Android capture 在相同位置重建出的峰值明显较低。

## 视觉与误差分布

- 两张 sRGB preview 的几何和光效外形肉眼接近。
- `absolute-difference-x8.png` 显示误差不仅存在于底部光效，也连续覆盖壁纸和图标，表现为全局红/蓝色度偏差。
- 最大底部误差只占很小面积，`> 0.05` 的像素在底部 436 行占 `2.47%`，但 HDR 峰值差异较大。
- 由于全局背景也出现连续色差，现阶段不能用全帧 MAE单独评估 Intelligent Light shader。

## 产物说明

- `android-source-gainmap.png`：Android `screencap` 原始 source PNG/gainmap。
- `android-private-reference.nfrimg`：写入剪贴板、供 Canvas 读取的 private payload。
- `android-private-linear.exr`：Canvas 同路径解码后的 premultiplied linear RGBA16F EXR，HDR 比较以此为准。
- `android-private-srgb-rgba16.png`：由 Android linear EXR 做 linear → sRGB 后生成的 16-bit/channel PNG，带 sRGB profile，供普通查看器使用；HDR 值仍会裁剪到 `[0,1]`。
- `node-forge.exr`：Node Forge headless freeze replay 输出。
- `node-forge-srgb-rgba16.png`：由 Node Forge linear EXR 生成的同规格 16-bit sRGB PNG，供视觉对照。
- `comparison-metrics.json`：完整数值、通道、百分位和空间分布统计。
- `android-preview-srgb.png` / `node-forge-preview-srgb.png`：查看用 sRGB preview。
- `absolute-difference-x8.png`：线性绝对差放大 8 倍后的可视化。

EXR 与 PNG 的边界不同：EXR 保存线性 HDR 数值并用于计算；PNG 保存经过 sRGB transfer 的显示值。不得把 linear sample 直接写进普通 PNG 后交给 sRGB 查看器显示。

## 要得到权威像素结论还缺什么

1. 提供并验证当前设备的 `tools/android-hdr-screencap/prebuilt/arm64-v8a/node-forge-hdr-screencap`，绕过 stock gainmap，直接抓 raw RGBA16F framebuffer。
2. 使用与 freeze fingerprint 完全相同的存档拓扑重新离线渲染，或重新冻结当前拓扑并让两端加载新 freeze。
