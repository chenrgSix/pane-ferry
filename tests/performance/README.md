# Mac ↔ Ubuntu 全链路延迟测量

在 Mac 上运行，复用项目已建立、验证过主机密钥的 SSH 登录：

```sh
python3 tests/performance/end_to_end.py \
  --host UBUNTU_IP --user UBUNTU_USER \
  --width 2560 --height 1664 --samples 30 --animated \
  --output /tmp/remoteapp-end-to-end.json
```

需要 Mac Swift/AppKit 和 Ubuntu 的 Agent 构建依赖、GTK 3 Python GI、GStreamer/OpenH264。
SSH 必须已通过 `scripts/connect-ssh.sh` 建立；脚本不处理密码或绕过主机密钥校验。
不传 `--animated` 可测静止窗口；传入时只有一个小块持续变化，主体为文字和图形。

WebRTC 对照先用 `scripts/build-webrtc.py` 构建两端的原生 runtime，然后运行：

```sh
python3 tests/performance/end_to_end.py \
  --host UBUNTU_IP --user UBUNTU_USER \
  --width 2940 --height 1612 --samples 30 --animated \
  --video-transport webrtc \
  --mac-webrtc-library /ABS/MAC/libremoteapp_webrtc.dylib \
  --remote-webrtc-library /ABS/UBUNTU/libremoteapp_webrtc.so \
  --output /tmp/remoteapp-webrtc.json
```

将 `--video-transport` 改成 `ssh` 测相同源码的控制组。两种模式使用同一采集、编码器、
分辨率、画质、输入、VideoToolbox 与 AppKit 绘制路径。`webrtc` 是强制模式，不能成功协商或
实际帧来自 SSH 时直接失败，不能把自动回退的结果标作 WebRTC。
可同时指定 `--mac-webrtc-bind`、`--remote-webrtc-bind` 固定两端媒体接口，避免比较时顺便更换网络路径。
每个有效 WebRTC 样本要求实际 UDP 候选对，并保留候选对以供核验。

`--video-transport auto --expect-transport ssh` 配合不可达的测试媒体地址，可验证启动回退；
`--video-transport auto --simulate-media-stall` 则只在临时 Mac 源码副本中，于第 9 次输入后
丢弃媒体回调，要求先通过 WebRTC 绘制、再通过 SSH 恢复同一窗口的输入回显。
后者会有故意制造的长样本，不纳入普通延迟比较；该丢帧逻辑不会编入正式产品。

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
`NSImage.draw` 完成，包含实际 SSH 控制/媒体或 WebRTC 媒体、H.264Decoder、60Hz inbox pump 和 NSView 重绘。
不强制提前 draw/flush，不替换解码器或绘制路径。终点尚未包含 macOS 合成、屏幕刷新扫描和
面板响应，不能将本结果称为高速摄像机测得的 input-to-photon 延迟。

所有差值都使用同一台机器的单调时钟。`ssh_network` 通过完整往返时间扣除两端内部耗时得到，
合计包含 SSH 输入与桥接、对应媒体传输、网络、协议处理、socket/流缓冲及少量序列化；
WebRTC 还包括 RTP 分片/重组、DTLS-SRTP 与帧完整性核验，没有把未经同步的两台时钟相减，
也不将其误称为单方向网络耗时。`response_to_capture` 包含应用重绘、等待重绘事件/采样时机以及
X11 读像素；`app_render`、`capture_wait`、`capture` 是进一步诊断项，可能有重叠，不重复相加。
其余标记阶段可精确加总回总耗时。`decode` 包括 JSON/base64/VideoToolbox/CGImage 准备，
`main_queue` 包括既有 60Hz pump 的等待；`draw` 包括提交给 NSView 后到绘制结束的时间。

结果含中位数、最近秩 P95、最小/最大值、逐样本数据与原始时间戳。标记、采样和日志会引入少量
开销；在相同环境/设置下比较。它是指定设备和测试窗口的实机测量，不代表所有应用、弱网、CI、
首次启动或生产环境的性能。每阶段中位数不一定相加等于总耗时的中位数。

两种传输在编码输出处使用相同计时点，WebRTC 的几何/流/帧 ID 和测试时间戳通过标准未注册 SEI
与对应 H.264 AU 一起走媒体轨道，没有逐帧等待 SSH 元数据。尺寸变更的控制消息仍走 SSH；
接收端保留最新已解码图像，等待匹配的几何版本，避免媒体先到时显示错尺寸。
`response_packet_bytes` 是应用层每帧接收负载：SSH 为 JSON/base64 消息，WebRTC 为重组后的
元数据和 H.264，不含完整网络头，不能把它当作物理链路带宽或单个 UDP 包大小。

2026-10-21 的远端优化对照保存于
[`docs/performance/2026-10-21-optimized-latency.md`](../../docs/performance/2026-10-21-optimized-latency.md)，
含八组统计及 720 个逐样本数据，也保留整体延迟未改善的首轮动态场景。
