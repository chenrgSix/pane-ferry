# Windows 低延迟流水线：实施与验证

在同日的 [竞品调研](2026-10-30-remote-streaming-research.md) 之后实施。保留 Windows 端合成主窗口、菜单和弹窗的行为，不更改配对身份或网络协议。

## 本轮完成

- 增加可选的分阶段计时：采集回读、场景几何、合成、色彩转换、编码、本地发送、输入排队/执行和反馈往返。`PANEFERRY_VIDEO_TIMING=1` 开启，每约 2 秒输出有界样本的 P50/P95/P99；不记录画面、标题、按键或输入文本。
- 发布端为每个接管会话创建独立采集线程。COM 采集和编码对象在该线程创建及销毁；输入线程只读取小型场景快照，仍然校验窗口身份、位置、所属进程和前台状态。最多一项采集任务在途，控制端慢时先停止生产，避免不断堆积编码帧。
- 使用 WGC `FrameArrived` 唤醒工作线程。若按帧率到点取帧时新帧尚未就绪，在独立线程有界等待新帧，避免多等整个发送周期。新系统可选设置 `MinUpdateInterval` 为 4 ms，最终发送帧率仍由协商值控制；旧系统不支持该属性时继续运行。
- 复用各窗口回读缓存、合成缓冲、NV12 缓冲和编码输出 sample。无新画面且布局不变时，不再每周期全量合成编码；保留有界静态质量刷新，关键帧请求和帧率切换可强制刷新。
- x86_64 支持 SSSE3 时使用向量化色彩转换，不支持时走原标量路径。随机奇偶尺寸、颜色和边缘测试逐字节核对，保持原有 BT.601、有限范围和奇数边缘复制规则。
- 使用有效视频反馈的额外延迟保守调整码率：连续三次超过基线 80 ms 才降低，单次抖动不降低；至少 1 秒调整间隔，稳定 5 秒后逐渐恢复。未知或重复确认不参与调整。保留 6 帧在途上限和关键帧恢复，不任意丢弃依赖帧。
- 可选系统硬件 MFT 编码路径，按能力枚举供应商，异步输入/输出事件有界等待，初始化失败尝试其他候选；运行失败转软件编码并使用新的流 ID / IDR。无外部 FFmpeg 运行依赖，不要求独显。
- Mac 增加接收至解码、解码排队、解码至提交显示的本机计时。继续使用现有 VideoToolbox、CVPixelBuffer 显示和最新帧合并，不调整窗口尺寸策略。

## 测量结果与边界

| 项目 | 修改前/中间版本 | 本轮结果 |
| --- | --- | --- |
| 2560×1440 CPU 色彩转换 P50/P95 | 5.31 / 6.52 ms | 代表性连续对照 1.94 / 2.31 ms；后续带其他负载复测约 1.9–2.4 / 2.3–3.1 ms |
| 同一动态 fixture，本地 RWP 240 帧 | 独立线程已拆分但仍固定轮询：53.05 fps，帧间隔 P95 33.51 ms | 新帧唤醒后 60.12 fps，帧间隔 P50/P95/P99 为 16.56 / 19.23 / 22.65 ms |
| 1602×932 真实动态窗口的代表性 P50 | — | 回读约 1.09 ms，合成 0.85 ms，转换 0.53 ms，编码 1.77 ms |
| 本地 JSON 发送 P95 | — | 动态 fixture 约 0.06 ms，当前不优先冒协议兼容风险重写本地 IPC |

上述 60.12 fps 是本地交付速率，仍不是 Mac 上屏率。另用真实采集线程在约 2.006 秒输出 120 个编码帧，FFmpeg 独立解码后读取测试窗口的二进制视觉计数器，得到 **111 个不同计数值**。所以不能把发送 60 帧宣称为每秒 60 张不同源画面；测试源自身绘制节奏、WGC 与显示刷新仍会影响独立画面数量。测试 fixture 已可持续复用来观察这个差异。

这次没有 Mac 运行环境，未在这里编译/运行 Swift 修改，也未实测 Windows→EasyTier→Mac 的端到端视觉延迟。Windows 反馈计时包含网络、接收处理、反馈批次等，不能当作纯网络 RTT。Mac 的提交显示回调不是物理屏幕扫描完成；跨设备单调时钟不能直接相减。

## 硬件策略与画质

默认继续软件编码，因为本机真实对照中软件编码约 3.7–4.3 ms，当前系统内存输入的硬件 MFT 路径约 5.6–5.7 ms；硬件路径尚未实现 GPU 场景合成及纹理直送，不能宣称零拷贝。

高级验证可在启动发布端之前设置：

```powershell
$env:PANEFERRY_VIDEO_ENCODER='auto'     # 枚举硬编，失败自动回退
$env:PANEFERRY_VIDEO_ENCODER='software' # 强制软件，也是未设置时的默认行为
$env:PANEFERRY_VIDEO_TIMING='1'         # 可选诊断
```

本机枚举到两个不能接受当前输出类型的 AMD 候选以及一个可用的 NVIDIA 候选。NVIDIA MFT 拒绝可选的 B 帧计数设置，因此使用禁止 B 帧的 Baseline；软件继续 High Profile。分别输出 120 帧，经 FFmpeg 独立解码验证均无 B 帧。相同合成测试首帧亮度 PSNR：软件 34.25 dB，硬件 36.00 dB，仅代表该测试首帧，不是总体画质排名。

仍然是 NV12 / 4:2:0 有损视频；“原画面尺寸”不等于无损画质。4:4:4、GPU 合成和纹理直送尚未实施，应在真实 Mac 计时/画质对照后继续评估。已有像素转换的加速不会改变输出像素，窗口尺寸策略没有改变。

## 验证

- `cargo test --locked -p paneferry-backend-windows -p paneferry-windows`：像素转换、场景合成、流控、几何变化及真实配对 QUIC 集成测试。
- QUIC 集成测试新增阻塞采集任务的场景，确认释放采集任务之前鼠标/键盘控制路径仍能完成；没有依赖短暂 sleep 假装编码并发。
- `real_capture_worker_streams_and_accepts_input`：专用动态窗口，持续视频、采集中点击、解除后再次接管。
- `capture_worker_routes_composed_menu_input`：新采集线程发现菜单，通过合成场景快照点击真实菜单并验证结果。
- `maximize_restore_and_activity_rate_changes_keep_capture_alive` 已切到新的发布端线程结构：六轮窗口尺寸和 60/1/5 fps 切换，验证新流 ID 和 IDR。
- 编码探针和动态窗口的独立 FFmpeg 解码，只保存专用 fixture/合成内容，不保存用户应用内容。

`tests/fixtures/windows-video.ps1` 是自动退出的独立测试窗口；`tests/performance/windows_stream_probe.ps1` 新增 `-StatePath` 和 `-WindowTitle`，支持在独立临时发布端上测试，输出本地交付和帧间隔分位数。

参考：[Microsoft 异步 MFT](https://learn.microsoft.com/en-us/windows/win32/medfound/asynchronous-mfts)、[MFT 枚举](https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mftenumex)、[WGC MinUpdateInterval](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.minupdateinterval)。
