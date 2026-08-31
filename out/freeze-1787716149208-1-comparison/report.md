# Freeze `freeze-1787716149208-1` Android / Node Forge 对比报告

生成时间：2026-08-26（Asia/Shanghai）

## 结论

- Android 与 Node Forge 输出尺寸均为 `1260 × 2800`，alpha 全部为 `1.0`，画面元素、Input Bar 和底部光效的位置一致。
- 之前 Android 使用错误 blend state 时出现的“只有一条细亮线”问题已经消失。当前两端的底部洋红光效形状基本一致。
- 剩余可见差异是：Android bloom 稍宽、稍亮；底部 436 行中，Node Forge 平均亮度比 Android 低约 `3.60%`。
- 全帧平均亮度非常接近：Node Forge / Android = `0.99770`。全帧线性 RGB MAE 为 `0.01396`；底部 436 行 RGB MAE 为 `0.00443`。
- 本次 Android 抓取不是 authoritative raw FP16：仓库缺少连接设备对应的 fingerprint-tested HDR helper，CLI 自动使用 stock gainmap PNG fallback，再重建为线性高精度数据。因此目前不能把剩余色差全部归因于 Android shader/render pipeline。
- 离线回放检测到当前 archive pipeline fingerprint 与 freeze 记录值不同；本次结果是“当前存档拓扑 + freeze 数据”的语义回放，符合 CLI 当前约定，但不是冻结时原始 pipeline 的字节级重建。

## 输入和生成方式

Android 先启动内置 replay：

```bash
adb shell am force-stop com.example.voice_interaction
adb shell am start \
  -n com.example.voice_interaction/.MainActivity \
  --es frame_replay doubao-voice-interaction-freeze-1787716149208-1
```

日志确认：

```text
loaded case=freeze-1787716149208-1
fingerprint=22c9ab811abb810e8fe826ba0a74b849f5c4b59ac240fae6ebca4f554f0ef94f
```

随后抓取 source-profile clipboard：

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

当前 archive fingerprint：

```text
92004da841ef4e345f3b7ddbd1634e5749aa7d21b34a6eb233f38a5e3411c72f
```

## 线性空间统计

| 指标 | 全帧 | 底部 436 行 |
|---|---:|---:|
| RGB MAE | 0.013960 | 0.004427 |
| RGB RMSE | 0.026325 | 0.018737 |
| 最大单通道绝对误差 | 0.798844 | 0.798844 |
| 平均亮度绝对误差 | 0.002073 | 0.000988 |
| Android 平均亮度 | 0.146763 | 0.016931 |
| Node Forge 平均亮度 | 0.146425 | 0.016321 |
| 最大通道误差 > 0.05 的像素 | 13.142% | 2.464% |
| 最大通道误差 > 0.10 的像素 | 1.855% | 0.988% |

图像范围：

| 输出 | RGB 范围 | 最大亮度 |
|---|---:|---:|
| Android gainmap 重建 | `-0.0721 … 1.4625` | 1.0132 |
| Node Forge EXR | `0.0000 … 1.9414` | 1.0000 |

Android 出现轻微负 RGB 是 Display-P3 / gainmap 转换到线性 sRGB 时保留广色域值的结果；Node Forge 输出最低值为 0。两端 RGB 极值不能仅凭最大值直接判断 bloom 亮度，应结合局部平均亮度和图像观察。

## 视觉判断

- 底部光效中心、Input Bar 尺寸和垂直位置对齐。
- Android 光晕横向扩散略宽，Node Forge 更集中；Node Forge 的底部区域平均亮度低约 3.6%。
- 放大 4 倍的差分图在整张壁纸和图标上也有连续色差。这与 Android 输出经过 HDR display/gainmap capture、Node Forge 输出为直接线性 EXR 的路径差异一致，不能视为 light-effect shader 的独立误差。
- 16-bit PNG 是常规查看文件，编码为 sRGB 并将线性值裁剪到 `[0, 1]`；HDR 数值对比必须使用 EXR。

## 后续建议

若要得到可作为渲染回归基准的权威结论，需要完成两件事：

1. 为当前设备提供并验证 `tools/android-hdr-screencap/prebuilt/arm64-v8a/node-forge-hdr-screencap`，重新抓取真正的 raw RGBA16F。
2. 明确是否接受“当前存档拓扑 + freeze 数据”。若要求冻结时原始 pipeline 严格复现，应先处理 archive/freeze fingerprint 不一致，再重新离线输出。

在这两个条件满足前，本次结果足以说明 Android blend 修复有效、几何和整体亮度已基本对齐，但不足以证明两端 HDR 像素完全一致。
