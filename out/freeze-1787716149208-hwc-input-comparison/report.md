# Freeze 1787716149208：Node Forge / Android HWC input 对比报告

生成时间：2026-08-27（Asia/Shanghai）

## 结论

Android HWC input 结合该 layer 的 Display-P3 dataspace 和 `3.0` HDR headroom，可以重建出与 Node Forge 离线 EXR 对齐的 linear-sRGB HDR 画面。

本次全帧最大单通道绝对误差（Max AE）为 `0.0329647064`，发生在 `(704, 2796)` 的蓝通道：Android 为 `1.7678165436`，Node Forge 为 `1.80078125`。全帧没有任何像素的 RGB 误差超过 `0.05`。

Android 重建峰值为 `1.9340636730`，Node Forge 峰值为 `1.94140625`，峰值数值差为 `0.0073425770`。在 Node Forge 峰值坐标 `(657, 2797)`，Android 为 `1.9318122864`，绝对误差为 `0.0095939636`。

因此，`HWC input + dataspace + headroom` 可以作为 Android 最终 app-to-HWC 边界的 HDR reference。它仍包含最终 Window `RGBA_8888` 的量化，但没有 stock screencap gainmap 路径中将峰值压到约 `1.14` 的问题。

## 输入与身份

- 用户指定 freeze：`1787716149208`
- 存档中的实际 case ID：`freeze-1787716149208-1`
- 名称：`Freeze 1787716149208`
- Freeze fingerprint：`22c9ab811abb810e8fe826ba0a74b849f5c4b59ac240fae6ebca4f554f0ef94f`
- 输出尺寸：`1260 × 2800`

Node Forge 离线渲染时报告当前 archive pipeline fingerprint 与 freeze fingerprint 不同，并按 semantic replay validation 继续执行。本次新生成的 Node Forge EXR 与前一次同 case 的 EXR 字节完全一致，SHA-256 均为 `8899b74f9c5206a51d7e24db6809a2bade18ebdeed29badc2e32e465b5d00cbf`。

Android 日志确认加载了同一个 case、时间为 `0.0`，并使用同一 fingerprint。Intelligent Light 创建并完成了 `84 × 84 RGBA_FP16 linear-sRGB` GPU intermediate。

## 1. Node Forge 离线渲染

```bash
node-forge-render-server \
  --headless \
  --nforge ../node-forge-editor/examples/doubao-voice-interaction.nforge \
  --freeze-case freeze-1787716149208-1 \
  --render-to-file \
  --output node-forge.exr
```

产物：`node-forge.exr`，`1260 × 2800`，linear HDR RGBA EXR。

## 2. Android replay 与 HWC input

Android 通过以下参数重放同一 freeze：

```bash
adb shell am force-stop com.example.voice_interaction
adb shell am start -W \
  -n com.example.voice_interaction/.MainActivity \
  --es frame_replay doubao-voice-interaction-freeze-1787716149208-1
```

为了取得可在本机直接解析的 input，只对新分配的应用 buffer 临时设置 `vendor.gralloc.disable_ubwc=1`，随后仅重启应用，没有重启 HWC。抓取后属性已恢复为 `0`。

```bash
hwc_dump.sh --no-restart -f 1 -l input --no-convert
```

同帧 SurfaceFlinger/HWC 信息：

- App layer format：`RGBA_8888`
- Raw stride：`1280 × 2816`
- 有效内容：`1260 × 2800`，crop origin `(0, 0)`
- Dataspace：`0x188a0000`，extended Display-P3 / sRGB transfer
- `current hdr/sdr ratio=3.0`
- `desired hdr/sdr ratio=3.0`
- HWC composition：`DEVICE`
- Gralloc：`compressed=false`

有效 `1260 × 2800 RGBA8` 内容 SHA-256 为 `6fc80c061871539ca1f4f7565be98490d11068615a02dee4c85cbc8dfa278378`。校验只覆盖有效内容，不包含右侧 stride padding；padding 未初始化，因此不应使用整个 raw 文件的 SHA 判断画面是否相同。

## 3. 本机 HDR 解算

Android 端只负责渲染与原始 HWC dump。以下转换全部由 Mac 上的 `reconstruct_hwc_input.py` 完成：

```text
HWC RGBA8
→ 归一化到 [0, 1]
→ sRGB EOTF
→ linear Display-P3
→ linear sRGB
→ RGB × HDR headroom 3.0
→ RGBA float EXR
```

执行命令：

```bash
python reconstruct_hwc_input.py \
  android-hwc-input_1280x2816_RGBA_8888_frame0.raw \
  --content-width 1260 \
  --content-height 2800 \
  --headroom 3.0 \
  --reference-exr node-forge.exr \
  --output-exr android-hwc-input-linear.exr \
  --output-nf16 android-hwc-input-linear.rgba16f \
  --metrics-output comparison-metrics.json
```

Headroom 不是唯一需要的信息：如果省略 `dataspace` 对应的 Display-P3 primaries 和 sRGB transfer，不能得到正确的 linear-sRGB 数值。

## 4. 对比结果

| 指标 | 结果 |
|---|---:|
| 全帧 Max AE | `0.0329647064` |
| Max AE 坐标/通道 | `(704, 2796) / B` |
| Android / Node Forge 最大误差点 | `1.7678165436 / 1.80078125` |
| R 通道 Max AE | `0.0261654854` |
| G 通道 Max AE | `0.0166785121` |
| B 通道 Max AE | `0.0329647064` |
| RGB RMSE | `0.0025855119` |
| RGB AE p99 | `0.0096980333` |
| RGB AE p99.9 | `0.0127855539` |

HDR 峰值：

| 图像 | 峰值 | 坐标/通道 |
|---|---:|---|
| Android HWC input 重建 | `1.9340636730` | `(684, 2797) / B` |
| Node Forge | `1.94140625` | `(657, 2797) / B` |
| 峰值数值差 | `0.0073425770` | — |

Android 与 Node Forge 的峰值都位于底部同一发光区域。由于 HWC input 已经过 RGBA8 量化和 P3 编码，单独取全帧 argmax 时坐标不必与 FP16 reference 完全相同；在 Node Forge 峰值坐标直接取样时，Android / Node Forge 为 `1.9318122864 / 1.94140625`，AE 为 `0.0095939636`。

像素阈值统计按“任一 RGB 通道超过阈值”计算：

| 阈值 | 像素数 | 占比 |
|---|---:|---:|
| AE > `0.01` | `74,622` | `2.115136%` |
| AE > `0.02` | `167` | `0.004734%` |
| AE > `0.03` | `7` | `0.000198%` |
| AE > `0.05` | `0` | `0%` |

## 判断

1. HWC input 没有把 HDR 峰值截断到 `1.0`。`RGBA_8888` 中保存的是经过 transfer、primaries 变换和 headroom 归一化后的 code value；结合 metadata 可以恢复到约 `1.93`。
2. 本次 HWC input 重建对 Node Forge 的全帧 Max AE 为 `0.032965`，与此前 app-owned Window FP16 对 Node Forge 的误差量级一致。主要残差仍位于底部光效，不是 HWC input 解码产生的全局颜色偏移。
3. Stock gainmap screenshot 的约 `1.14` 峰值不能代表 Android renderer 或 app-to-HWC 数据；HWC input 重建结果证明实际提交给 HWC 的数据包含正确 HDR headroom。
4. 该边界适合验证“Android 应用最终提交给 HWC 的一帧”。它不包含 LM、DSPP 和面板校准之后的变化；验证最终显示后端仍需另外看 HWC output tap。

## 产物

- `node-forge.exr`：Node Forge 离线 freeze replay。
- `android-hwc-input_1280x2816_RGBA_8888_frame0.raw`：Android HWC input 原始数据。
- `android-hwc-input-linear.exr`：本机解算后的 Android linear-sRGB HDR EXR。
- `android-hwc-input-linear.rgba16f`：相同数据的 NF16 half-float 版本。
- `comparison-metrics.json`：完整数值结果。
- `android-render.log`：Android replay/render 日志。
- `surfaceflinger.txt`：同帧 SurfaceFlinger/HWC 元数据。
- `android-preview-srgb.png`、`node-forge-preview-srgb.png`：查看用预览。
- `absolute-difference-x16.png`、`absolute-difference-bottom500-x16.png`：线性绝对差放大 16 倍后转换为 sRGB 的预览。
