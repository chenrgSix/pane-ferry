# Mac 接收画质对比（2026-10-23）

H.264 保持默认；PNG 是可选的无损对照，用来区分压缩损失与源分辨率不足。PNG 不能给 1 倍源图补出 Retina 细节。

## 使用方式

远程窗口的“显示”菜单选择“流畅优先（H.264）”或“清晰优先（PNG 无损）”。切换短暂重连同一会话，保留本机窗口和远端应用；选择会记住供后续连接使用，其他已打开连接不变。窗口大小、比例、输入坐标和应用缩放算法均未修改。

## 本地隔离样本

Apple M4、macOS 27.0.1。相同 1470×802 源图，60 帧文字/控件和小范围动画；先预热，每种模式测 3 次。复用生产 ImageIO / VideoToolbox 解码与 SurfaceView 绘制，离屏输出 2940×1604。H.264 样本用本机 FFmpeg 8.1.1 libx264 veryfast / yuv420p 生成，并非 Ubuntu OpenH264。

| 指标 | PNG 无损 | H.264 样本 |
| --- | ---: | ---: |
| 与首帧源图的 RGB 平均误差（0–255） | 0.000 | 0.844 |
| 平均解码耗时 | 4.23 ms/帧 | 3.40 ms/帧 |
| 平均解码＋离屏绘制 | 30.88 ms/帧 | 26.27 ms/帧 |
| 持续 30 帧/秒的估算载荷 | 9.37 MB/s | 0.23 MB/s |

数据量包含协议中的 Base64，不含 JSON、QUIC 和链路开销；静止画面并不会持续发送 30 帧。处理耗时不包含网络、远端采集/编码、JSON 解析、屏幕合成，不等于实机帧率或输入延迟。均值包含对整帧空白区域的统计，RGB 误差不能直接当作可读性评分。测试时没有并行运行项目门禁，但没有冻结用户其他 Mac 进程。

本样本中，PNG 的数据量约为 H.264 的 41 倍，且解码和绘制略慢。这支持保持 H.264 默认，PNG 只作画质对照；不能据此推断所有应用的固定倍数，也不能宣称真实飞书/GPT 已改善。

## 原始数据与图片

- [逐次结果、条件与安装校验](validation-artifacts/2026-10-23-png-quality.json)
- [PNG 解码样本](validation-artifacts/2026-10-23-quality-png.png)
- [H.264 解码样本](validation-artifacts/2026-10-23-quality-h264.png)

复现：

```sh
python3 tests/performance/mac_display_quality.py --output /tmp/paneferry-quality-comparison
```

本轮只更新 Mac WindowHost。Ubuntu 未连接、未重启或注销，窗口桥保持此前停用状态；账号和远端任务没有被本轮操作。实际 Ubuntu 画面和长期流畅度仍待用户安排验收。
