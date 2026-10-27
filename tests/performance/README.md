# Mac ↔ Ubuntu 全链路延迟测量

## 已授权实际应用的右键菜单

`popup_end_to_end.py` 用于用户明确授权的实际应用（例如 IDEA），不使用 GTK 计数夹具。
先确认源端运行桥接版本 3、应用没有待处理对话框，观察画面后选择不会执行操作的右键位置。
探针沿用正常接入后提供的源尺寸与 DPI，不另指定测试分辨率；普通接入当前请求 1×，早期版本曾强制 2×，应记录实测密度。
仅注入右键和 Escape；不提交文字、不选择菜单操作、不读剪贴板。
它编译当前 WindowHost 的临时副本，使用原有 QUIC、VideoToolbox 和 AppKit 路径，不修改安装包。

```sh
python3 tests/performance/popup_end_to_end.py --build-dir /tmp/popup-probe --build-only
python3 tests/performance/popup_end_to_end.py --build-dir /tmp/popup-probe \
  --peer PAIRED_DEVICE_ID --state /absolute/network-state --helper /absolute/paneferry-network \
  --app APPROVED_WINDOW_ID --output /tmp/popup-result.json --command-file /tmp/popup-command.json
```

探针创建 `/tmp/popup-result.json.state.json`，报告 ready、实际尺寸、父子关系、显示区域及窗口样式。
观察主窗口后，原子写入命令文件，例如 `{"serial":1,"action":"preview","x":200,"y":200}`；
x/y 为源视频像素坐标，不能照抄未经观察的示例。preview 保持测试菜单供视觉检查；
`{"serial":2,"action":"close"}` 仅用 Escape 关闭该菜单。随后用新 serial、action:run、相同坐标、
samples:30，以及刚刚观察确认的菜单视频像素尺寸 popup_width/popup_height 开始正式测试；
每轮两次预热后测指定次数。临时命令文件只由测试操作者写入。

ready 或 preview 阶段可发送新 serial、`action:snapshot`，将当前可见窗口的解码图像与几何元数据
导出到结果路径加 `.snapshot/` 的本机私有目录。只在明确请求时导出，避开计时阶段；图像可能包含
应用内容，不应提交到仓库。这是流内图像，不是 macOS 最终桌面合成截图。

每个样本必须等到本次新开的、属于所选主窗口且匹配已确认尺寸的无边框菜单实际绘制完成；普通主窗口帧、
输入前已存在的子窗口和其他尺寸浮层不会结束计时。部分应用的提示浮层也被 GNOME 标为 menu，不能仅按类型计时。
当前菜单关闭后才能触发下一次输入，超时、菜单仍带标题栏或意外已有子窗口会终止测试并报告失败。
正式结果强制核对 H.264 与硬件解码。预览与失败尝试不能混入成功样本。
数据只代表该应用右键菜单打开的延迟，包括临时窗口的采集和编码器启动；不能外推成打字、滚动或所有操作的延迟。
当前 CoreVideo 路径的终点见下一节；旧版位图记录的终点为 AppKit 绘制返回。两者均未包括显示扫描与面板响应。

## CoreVideo 显示路径的计时终点

当前 Mac 宿主保留 VideoToolbox 的 CoreVideo 缓冲，使用 AVFoundation 视频层显示，不在正常播放时逐帧转 CGImage。
临时探针通过 `displayedPixelBuffer()` 核对当前提交的 CVPixelBuffer 或 IOSurface，记录显示缓冲可用时间；
之后才检查 ROI/视觉标记，避免将像素识别开销计入终点。菜单探针直接把已观察确认的 ROI 缩为
16×16 像素进行比对，预览和关闭基线也使用同样的 ROI 路径，不逐帧回读整张图像。需要 macOS 14.4+。
显式设置 `PROBE_SNAPSHOT_ON_FAILURE=1` 可在失败后导出私有诊断截图；截图不参与计时，也不要提交到仓库。
结果的 endpoint 为 `renderer displayed-buffer availability`，不是旧版 `NSImage.draw` 返回，
也不包括最终屏幕扫描和面板响应。不同终点的历史总数不能作为严格的同条件 A/B。

## 发布端帧率上限试验

Linux Agent 可通过 `PANEFERRY_VIDEO_MAX_FPS=30` 限制视频上限，取值为 1–90 的整数；未设置时保持原协议能力。
设置在 Agent 服务环境中，重启后生效。它会向客户端公布上限，并限制 `video.enable` 与后续 `video.configure`，
因此自动协商或重连不会把上限升回 60。`video.ready.max_fps` 返回实际接受的上限。
这改变采集/编码发送节奏，不改变 GNOME 虚拟显示器的 60 Hz、源像素尺寸、缩放或 QP 上限；
编码器的名义码率仍随协商帧率计算，因此不能把 30/60 上限试验描述为完全相同码率的对照。
静止画面去重与拥塞时自适应降帧仍保留。删除环境覆盖并重启可恢复自动协商。

## Agent 阶段诊断副本

`python3 tests/performance/agent_stage_probe.py /tmp/fresh-agent-copy --revision HEAD`
从指定 Git 提交创建独立源码副本，加入右键输入接收、分发、处理完成、取帧、编码和写出时间戳。
它不修改工作树或安装包，也不自动部署。仅用于已授权的单控制器、顺序右键测试；诊断结束应恢复正式 Agent。
`_agent_stages` 会随对应帧进入 PopupProbe 记录，不包含按键内容或点击坐标。

`raw_first/raw_last`、`converted_first/converted_last` 是本次输入后各 appsink 的首个/最近回调，
用于判断管线是否已就绪；不是与最终匹配帧严格绑定的原始帧时间戳，不能当作精确应用绘制耗时。
`pull_first` 是输入后首次尝试取帧，`interval` 是当时自适应发送间隔。所有 Agent 时间戳来自 Linux
CLOCK_MONOTONIC；只在同一端做差。完整 Mac 响应区间扣除 Agent 内部区间可估计双向传输与桥接残差，
不能把两台机器的绝对时间戳相减。插桩存在开销，最终结果还需用无插桩正式构建复测。

选帧诊断的 `--trace-only` 模式只写本地 JSONL，不把 `_agent_stages` 放进协议。
其中 `ready`、`raw_pull`、`i420_pull` 分别保留样本身份、就绪时间和实际读取时间，
避免把首个响应样本与最终编码样本混为一谈；`status_start/status_end` 单独记录输入状态查询。
`rpc_queued/rpc_started/rpc_completed` 用同一个本地 RPC ID 拆开排队和执行，
区分 foreground、metadata、bridge 和 desktop；只记录操作名和测试计数，不记录输入内容。
X11 兼容路径也记录 capture 和编码边界；其编码前不是 I420，因此 selection 字段可为空。
夹具的 `after_paint` 只表示 GTK 绘制阶段结束，不等于 GPU 完成或屏幕已显示。
Xwayland 保持 2× backing、输出 1× 时可加 `--output-markers`，让视觉计数的位置随输出归一化；
源端真实尺寸和缩放仍应单独核验。

## 网络与显示边界诊断

`python3 tests/performance/network_stage_probe.py /tmp/fresh-network-copy --revision HEAD`
生成独立源码副本；在副本中构建 `paneferry-network`，并在两端临时服务环境中设置
`PANEFERRY_NETWORK_PROBE_TRACE=/private/directory/network-trace`，得到按 PID 分开的
0600 JSONL 文件。该构建不自动部署，不改变协议字段，也不记录视频负载。
使用隔离的网络状态目录，结束后停止自建服务并清理复制的配对凭据。

以 `session_id + frame_id` 关联发送端 `packet_assembled/quic_write_begin/quic_write_complete`
与接收端 `quic_read_complete/packet_restored/json_write_complete`。
QUIC 写出完成表示本地异步写入完成，不是远端收到；`first_byte_ns` 是读取到包首字节后的
本地时间，不是网卡硬件时间戳。Mac 服务端的 JSON 写出终点是到本地 bridge 的 Unix socket，
后续 bridge 到探针完整行的交付仍须用探针 `received` 计时。
Linux 用 CLOCK_MONOTONIC，Mac 用 CLOCK_UPTIME_RAW，与 Swift uptime 对齐；跨端绝对值不能相减。
关键帧附近另采样 `quic_progress`，保留发送流 ID、消息起始字节偏移、窗口、ACK、
丢包、流控阻塞和 UDP 字节计数。它是定时统计采样，不是逐包 qlog，也不直接提供 bytes-in-flight；
一次采样中的字节增量可能包含相邻视频消息，不应全归给当前关键帧。

显示探针记录最后一次未匹配与第一次匹配查询的起止时间、预期和实际 IOSurface ID。
它们约束的是 renderer 缓冲查询的观测窗口，不能当作精确 presentation 时间或面板亮起时间；
没有前一次失败查询时，不推定额外下界。正式对照和两端插桩的诊断结果应分开统计。

## 当前版本：已部署 Agent 的 Wayland/QUIC 实测

`live_end_to_end.py` 构建当前 WindowHost 的临时计时副本，连接已配对的生产网络服务和
已安装的 Agent。服务端不插桩、不使用 Xvfb。只将 `--app` 指向测试自有窗口，勿选择用户应用。
需事先确认 Ubuntu 已登录 Wayland、安装 GTK 3 Python GI，并已授权 PaneFerry 输入。

在 Ubuntu 的桌面用户环境启动 `wayland_latency_fixture.py`：

```sh
GDK_BACKEND=wayland python3 wayland_latency_fixture.py /tmp/fixture.jsonl 2940 1618 idle 2
```

最后的 `2` 是源显示器缩放；`idle` 可换成 `animated`（每 33 ms 重绘，小块变化）。从 Agent
`window.list` 取标题 `PaneFerry Live Latency Fixture` 的 `wayland-...` ID。每轮重新启动夹具，
确保画面计数从 0 开始；若跨 Agent 重启，夹具应由独立进程或独立 user unit 运行。

```sh
python3 tests/performance/live_end_to_end.py --build-dir /tmp/paneferry-live-probe --build-only
python3 tests/performance/live_end_to_end.py --build-dir /tmp/paneferry-live-probe \
  --peer PAIRED_DEVICE_ID --state /absolute/path/to/network-state \
  --helper /absolute/path/to/paneferry-network --app wayland-PID-START-WINDOW \
  --width 2940 --height 1618 --scale 2 --fps 30 --samples 40 --output /tmp/live-result.json
```

`--scale` 只覆盖临时探针副本的启动请求，不修改已安装程序。对照分辨率时，固定同一个帧率，
交错运行 `--width 1470 --height 809 --scale 1` 与 `--width 2940 --height 1618 --scale 2`。
每轮重启自有夹具并同时传入对应像素宽高和缩放；给夹具添加 `--logical-content`，保持两组
相同的逻辑页面、文字大小和本机显示区域，避免把不同布局当成密度收益。黑白计数标记仍用固定
物理坐标。源端每轮核对输入顺序、绘制尺寸和实际缩放；探针结果核对协商的帧率。

CoreVideo 计时先记录对应缓冲已显示的时间，再直接读取 NV12 亮度平面的 8 个标记点，
避免正常采样时为识别标记回读整帧 CGImage。验证此识别与计时边界可运行
`python3 tests/performance/live_probe_unit.py`，无需连接远端或注入用户应用。

方向键与点击交错，各预热 2 次、正式采样 40 次；8 位视觉标记必须匹配才记录。每轮核对
远端 JSONL 中的输入计数、输入种类、绘制尺寸与缩放，保留原始数据及失败轮次。
总延迟为同一 Mac 单调时钟的 `(draw_end - start) / 1e6` ms；当前 CoreVideo 路径中保留的
`draw_end` 字段表示上述显示缓冲可用时间，历史位图记录则表示 AppKit 绘制完成，
不含硬件输入、macOS 合成、屏幕扫描和面板响应。`received` 是完整协议行开始解析的时间，
不能当成首个网络字节到达时间。程序强制核验 H.264 与 VideoToolbox 硬件解码。
运行后关闭测试窗口，清理自建进程；源码和已安装的 WindowHost 不会被修改。

## 历史：隔离环境的 SSH/QUIC 对照

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
