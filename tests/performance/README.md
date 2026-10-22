# Mac ↔ Ubuntu 全链路延迟测量

> 历史实验复现：SSH/QUIC 对照基于 `c927116`（移除 SSH 前）的代码与夹具。请在该提交的独立检出中运行下列命令；当前产品已移除 SSH 适配器和密码助手，不再满足这些历史源码插桩位置。原始数据与结果保持不变。

在 Mac 上运行，复用项目已建立、验证过主机密钥的 SSH 登录：

```sh
python3 tests/performance/end_to_end.py \
  --host UBUNTU_IP --user UBUNTU_USER \
  --width 2560 --height 1664 --samples 30 --animated \
  --output /tmp/paneferry-end-to-end.json
```

需要 Mac Swift/AppKit 和 Ubuntu 的 Agent 构建依赖、GTK 3 Python GI、GStreamer/OpenH264。
SSH 必须已通过 `scripts/connect-ssh.sh` 建立，也可用 `--control-path /path/to/existing-master.sock` 指定已有连接；脚本不处理密码或绕过主机密钥校验。
不传 `--animated` 可测静止窗口；传入时只有一个小块持续变化，主体为文字和图形。

默认业务通道为 `--transport ssh`。对照 QUIC 时添加：

```sh
--transport quic --network /absolute/path/to/release/paneferry-network
```

QUIC 模式在两端建立临时网络实例与一次性配对，输入和视频只走 QUIC；SSH 仅用于上传/构建测试副本、管理临时进程和取回日志。邀请只经过已认证的管理管道，不出现在命令行或测量报告。两端网络组件使用 release 构建；Ubuntu 临时网络副本仅在读取状态时附加 RTT、包丢失和字节计数，不改变传输参数。报告记录 `transport`；SSH 与 QUIC 应按交错顺序在相同尺寸、画质与输入样本数下比较，不能只留下更快的轮次。

脚本在临时目录给当前源码副本加入计时钩子，编译独立 Agent 与原生 WindowHost。
独立 HOME/catalog/socket/Xvfb 仅启动自己的 GTK 测试程序；不会替换已安装的二进制、
重启已有服务、控制现有应用或访问系统剪贴板。测试结束（包括失败）后停止自有进程并清理目录。
可指定 `--remote-build-cache /tmp/TASK_OWNED_TARGET` 复用本次任务的 Cargo 构建目录。
该目录里的二进制包含测试钩子，不要部署为正式 Agent。
可用 `--mac-module-cache /tmp/TASK_SWIFT_MODULES` 复用本次任务的 Swift 模块缓存。

每种输入先预热两次，再采样指定次数，交错测试方向键、文字提交和点击；采样间随机间隔避免
总在采集定时器的同一相位触发。每次输入让远端画面上的 8 位黑白标记递增；只有 Mac 实际
绘制了该输入 ID 的图像才完成一个样本。后台动画、上一帧、仅收到输入或仅收到视频包均不算完成。
另用远端记录核验输入种类、计数顺序，并要求实际 H.264 和 VideoToolbox 硬件解码。
测试副本只接受夹具自己注入的键、文字和左键按下，过滤非测试输入；GTK 的额外双击/三击
通知也不会被重复计数。观测到计数跳跃会直接拒绝本次测量。

起点是调用产品 `SurfaceView` 的输入处理函数；文字从 `insertText` 开始，因此包含提交后的
完整传输链路，不包含用户拼音组合的时间、键盘硬件扫描与系统派发。终点是对应图像的实际
`NSImage.draw` 完成，包含公共 RWPTransport、H.264Decoder、60Hz inbox pump 和 NSView 重绘。
不强制提前 draw/flush，不替换解码器或绘制路径。终点尚未包含 macOS 合成、屏幕刷新扫描和
面板响应，不能将本结果称为高速摄像机测得的 input-to-photon 延迟。

所有差值都使用同一台机器的单调时钟。`transport_network`（旧记录名为 `ssh_network`）通过完整往返时间扣除两端内部耗时得到，
合计包含选定传输、桥接、网络、socket 写出、流读取端缓冲及少量 JSON 序列化；没有把未经同步的两台时钟相减，
也不将其误称为单方向网络耗时。`response_to_capture` 包含应用重绘、等待重绘事件/采样时机以及
X11 读像素；`app_render`、`capture_wait`、`capture` 是进一步诊断项，可能有重叠，不重复相加。
其余标记阶段可精确加总回总耗时。`decode` 包括 JSON/base64/VideoToolbox/CGImage 准备，
`main_queue` 包括既有 60Hz pump 的等待；`draw` 包括提交给 NSView 后到绘制结束的时间。
`response_packet_bytes` 是到达宿主的兼容 JSON 行长度，QUIC 在本机已恢复 base64，不能把这个字段当作物理网卡流量或直接比较网络带宽。

结果含中位数、最近秩 P95、最小/最大值、逐样本数据与原始时间戳。标记、采样和日志会引入少量
开销；在相同环境/设置下比较。它是指定设备和测试窗口的实机测量，不代表所有应用、弱网、CI、
首次启动或生产环境的性能。每阶段中位数不一定相加等于总耗时的中位数。

2026-10-21 的远端优化对照保存于
[`docs/performance/2026-10-21-optimized-latency.md`](../../docs/performance/2026-10-21-optimized-latency.md)，
含八组统计及 720 个逐样本数据，也保留整体延迟未改善的首轮动态场景。
