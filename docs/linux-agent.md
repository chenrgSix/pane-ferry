# Linux Agent 开发版

普通用户可直接使用[可双击安装的 Debian 包](debian-package.md)，安装、配对与日常使用见[使用教程](usage.md)。下方保留源码和用户目录安装说明。

普通用户运行，不更改桌面全局缩放。X11 独立会话运行依赖：Xvfb、Openbox、xauth、dbus-run-session；应用发现另使用系统 Python GI、GIO/GioUnix、GTK3 和 GdkPixbuf。不会自动安装系统依赖、启用 linger、改 SSH 配置或防火墙。原生 Wayland 支持另需下述 GNOME 窗口扩展。

HexHub 等显式采用桌面密钥服务兼容策略的入口另需 `xdg-dbus-proxy` 和当前用户已登录的桌面总线；`paneferry-agent check` 会报告代理是否可用。该策略只开放 Secret Service，详见[应用启动兼容](application-discovery.md#启动)。

## 手工安装

1. 编译：`cargo build --release -p paneferry-agent`。
2. 将二进制安装到 `~/.local/bin/paneferry-agent`（完整产品安装还包括 paneferry-network 和 GTK 管理界面）。
3. 默认发现当前用户已安装的桌面应用。可选的 `~/.config/lan-remoteapp/apps.json` 用于自定义入口，格式参考 `deploy/apps.example.json`；审核其中的绝对路径及 argv。没有此文件也能运行。应用仍具有当前用户权限，目录不是 OS 沙箱。
4. 前台运行 `~/.local/bin/paneferry-agent serve`。或将 service 文件放入 `~/.config/systemd/user/`，由用户自行执行 `systemctl --user daemon-reload` 和 `systemctl --user enable --now remoteapp-agent.service`。
5. 两端打开管理界面，用设备邀请和地址完成配对后连接，见[统一传输指南](native-transport.md)。

状态目录 `~/.local/state/lan-remoteapp` 权限 0700；agent.sock 0600。Xvfb 使用共享用户 Xauthority 中每显示独立随机 cookie，关闭 TCP。发布器只提供私有 Unix socket；网络组件监听 UDP 42424，通过 QUIC 设备证书验证对端，不需要 SSH 账号。

Linux 开发包由 `bash scripts/build-linux.sh` 生成，包含双向使用的 Agent、网络组件、管理/接收界面和 GNOME 窗口扩展。已有服务运行时，普通 `./install.sh` 在写文件前拒绝升级，避免停止其拥有的独立应用。使用 `./install.sh --defer-restart` 原子替换二进制并安装扩展、更新 service，保留当前服务和应用；它只执行 daemon-reload，不 stop/start/restart。保存工作后再由用户执行 `systemctl --user restart remoteapp-agent.service`。首次 GNOME 扩展安装可能要等下次登录才生效，不能用安装成功代替功能已激活。

## 窗口视频流

新版 Mac 宿主优先协商 H.264。Agent 的独立输出线程运行系统 GStreamer 的 appsrc、videoconvert、openh264enc、h264parse、appsink 管线，奇数宽高才插入 videobox 补边。通过稳定 C API 运行时加载 `libgstreamer-1.0.so.0` / `libgstapp-1.0.so.0`，视频编码不再依赖 Python GI 或原始帧 socketpair。只读像素缓冲由 Arc 租约保持至 GStreamer 释放，避免跨进程传帧、Python 缓冲拼接和再次填充 GstBuffer。identity 只按原 sequence/30 规则设置时间戳；颜色转换及编码器、码率、GOP、QP 上限、帧跳过策略保持不变。拉取编码结果有 2 秒超时，失败维持现有 PNG 回退；停止管线后释放缓冲及对象引用。不运行客户端提供的管线或命令。

缓冲共享遵循 [GStreamer 的 wrapped-buffer 生命周期约定](https://gstreamer.freedesktop.org/documentation/gstreamer/gstbuffer.html#gst_buffer_new_wrapped_full)：数据保持只读，释放通知回收租约，不依赖私有结构体布局。`tests/performance/legacy_video_encoder.py` 仅用于与旧管线比较解码像素，不再作为运行时编码进程。

编码使用 OpenH264 屏幕模式、无 B 帧、关键帧加 P 帧；窗口改变大小、隐藏恢复或重连后从新的关键帧开始。仅在编码之前合并旧画面，已编码的依赖帧不丢弃。实际显示像素尺寸保持不变，奇数宽高为编码补至偶数，Mac 解码后裁掉补边。编码为有损 4:2:0，文本颜色精度与 PNG 不同。

Agent 在 hello 中仅在组件可用时声明 `native.h264`；没有组件或编码失败时保留 PNG，Mac 副标题明确显示回退。视频经 QUIC 可靠流传输，不使用 WebRTC；不会配置 STUN、TURN、公网端口或更改防火墙。Ubuntu 已有组件在实机测试中可用，但其他 Linux 发行版需要自行安装运行依赖。

## 接入现有 X11 窗口

启动器的“运行中的桌面窗口”会自动读取当前用户可接管的窗口；点击后只接入选中的主窗口及关联弹窗，不需要手填 PID。读取列表只查询元数据，不改变原桌面窗口。应用、图标、身份校验与 Wayland 限制见 [应用与窗口发现](application-discovery.md)。以下 existing 配置保留为兼容的固定进程入口，范围仍为整个已批准进程。

在服务端白名单中为指定进程添加 `existing`。以下值仅为格式示例，PID、显示号和授权文件路径必须由 Linux 用户核对：

```json
{
  "id": "existing-app",
  "name": "现有应用窗口",
  "executable": "/absolute/path/to/application",
  "argv": [],
  "existing": {
    "display": 0,
    "xauthority": "/run/user/1000/approved-Xauthority",
    "process_id": 12345
  }
}
```

该路径不会启动应用、Xvfb 或窗口管理器。Agent 核验进程 UID、可执行文件和启动时间，按 `_NET_WM_PID` 筛选窗口及其有明确父子关联的弹窗，使用 XComposite 获取合成器重定向的窗口图像。客户端只能提交 app_id，不能指定任意 PID、显示号或授权文件。认证 cookie 仅在 Agent 内存中使用，不传给 Mac。

同一进程的重复连接请求复用会话，并保持单控制端限制。断开或停止 Agent 不结束此类原有进程；关闭 Mac 窗口只断开连接并退出宿主，保留原窗口。原进程退出后需要在本地重新核对并更新 PID。原生 Wayland 窗口不支持此方式；现有桌面上的尺寸和输入会作用于原窗口。

应用目录会标明已退出或不匹配的现有进程，启动器禁用该入口并解释原因；不会自动接管同名的新进程。请求期间进程退出会返回 `existing_process_unavailable`。已结束/回收的会话分别返回 `session_exited` / `session_expired`，启动器在明确拒绝后清除旧启动请求 ID；网络超时和未知错误仍保留 ID，避免重试重复启动。

`existing.scale_factor` 可选，默认为 1，用于兼容的初始窗口尺寸换算，不是对实际 DPI 的测量；标题标记沿用原应用缩放。新独立会话的 `app.launch.scale_factor` 会配置 GTK/Qt 缩放；受支持的飞书桌面入口另传 Chromium `--force-device-scale-factor`，只对新实例生效，尊重已有显式参数。标题将其标记为启动偏好，不能把设置参数等同于应用确实采用。GTK 已实测，当前飞书 2×、Qt 和传统 Xlib 的视觉效果仍需逐项验证。Agent 不更改原桌面的全局 DPI，也不为更新缩放退出已有应用。

`deploy/firefox-x11.sh` 仅保留为显式隔离测试工具，不是接管原 Firefox 的产品路径。原窗口、账号与标签页通过下述 Wayland 接管路径保留，不复制或创建替代浏览器配置。

## ChatGPT 页面高清与虚拟显示器

新增的可选适配器支持已准备调试入口的 `/usr/lib/chatgpt/ChatGPT` XWayland 主窗口，源密度必须为 1×，依赖 `/usr/bin/python3`、`python3-websocket` 和 libX11。它通过应用页面自己的布局缩放变量和浏览器 WebView CSS 布局，在目标客户区增大后重新绘制；不修改 XSettings、全局 DPI 或同进程其他主窗口。仅验证当前布局版本，未知布局明确拒绝。

应用没有调试入口时，在 Ubuntu 本地显式运行 `paneferry-agent prepare-chatgpt PID`。命令校验同 UID、准确可执行路径和原进程启动参数；仅对普通无额外参数的启动发送 SIGTERM，等待退出后用原 profile 重新启动，并添加 `--remote-debugging-address=127.0.0.1 --remote-debugging-port=0`。不强杀、不复制账号；已准备的进程直接返回成功。调试入口具有应用控制能力，仅接受该主进程持有的回环监听端口，远程协议不暴露 CDP。此命令会重启整个 ChatGPT 进程，应先保存该应用的工作。

新版 Mac WindowHost 接管受支持的目标 ChatGPT 主窗口后自动开启 2× 页面高清，重连后自动重新开启；2× 同时将目标迁移到同会话的私有虚拟显示器，1× 不迁移。在“显示 → ChatGPT 页面高清（2×）”可对本次连接手动关闭。关闭高清、关闭控制窗口或连接断开会恢复页面、原位置和大小，并释放虚拟显示器。虚拟输出需要 Python GI/Gio 和 Mutter RecordVirtual，已验证 GNOME 50；创建失败时恢复并拒绝本次高清。此默认策略仅作用于目标 ChatGPT 页面，不覆盖其他应用、系统原生菜单/文件对话框，也不提供独立输入席位。完整边界和实测见 [ChatGPT 页面高清](chatgpt-page-scale.md)。

## 接入现有 GNOME Wayland 窗口

支持 GNOME 50（当前在 50.1 隔离桌面测试）。将 `apps/linux-agent/gnome-extension/` 安装到用户的 `gnome-shell/extensions/paneferry@local/` 并启用。普通本地 D-Bus 窗口接口提供元数据、目标窗口的尺寸/焦点操作与租约；不启用 Shell Eval，不改变其他用户、全局缩放或桌面分辨率。当前是同用户组件间的范围校验，不是额外的操作系统沙箱。

Agent 保留原 PID 和窗口身份，不启动新 Firefox、不访问浏览器资料。主窗口与明确 transient 弹窗通过 Mutter RecordWindow / PipeWire 分别采集，使用 `GstVideoCropMeta` 去掉合成器 padding，再送同一 PNG/H.264 输出。另需 `libgstvideo-1.0.so.0`、GStreamer `pipewiresrc`、运行中的 PipeWire 和 Mutter ScreenCast。共享输入授权仍使用 RemoteDesktop Portal；文字复制/粘贴经 GNOME 文本剪贴板桥接，当前拒绝二进制剪贴板和剪切，不在后台读取。

GNOME 50 的 RecordWindow 使用显示器大小的缓冲区；原生 Wayland 目标超出可完整采集区域时明确返回 `resolution_limited`，不会冒用目标尺寸或放大低分辨率帧。尺寸请求先从像素换算到合成器逻辑单位。Xwayland 仍用 XComposite 原像素，扩展通过用户尺寸请求允许大于物理屏幕的目标窗口，并仅移动选中窗口，使画面外控制点能落入输入设备范围。比例、预算和实测边界见[分辨率说明](window-resolution.md)。

首次安装扩展时，GNOME 可能直到下次登录才发现它。不要为自动验收注销桌面；注销会退出用户应用。Agent 的独立会话也不持久化，重启服务前须保存工作。普通 X11 路径在扩展未启用时保持可用，但原桌面的尺寸请求仍可能被窗口管理器限制。

### 原桌面统一输入授权

Ubuntu Wayland / Xwayland 原桌面的键鼠由 **窗渡 PaneFerry Agent** 统一申请和管理。首次接管时，在 Ubuntu 的系统窗口确认控制授权并保留“记住”选项；授权的是同一 Ubuntu 用户的 Agent。之后新增应用、关闭控制端窗口、网络断线和重新连接，都复用 Agent 的同一个输入会话，不按应用另弹分享窗口。应用列表发现、只准备会话（`attach:false`）和独立 Xvfb 应用不申请原桌面授权。

输入管理器使用 RemoteDesktop Portal v2 的 `persist_mode=2` 与系统 libei，只申请键盘和指针。原生 Wayland 的窗口采集由独立的 Mutter 窗口流负责，不申请整个显示器的采集；Xwayland 保持 XComposite。所有输入仍先核验所选窗口、进程身份和窗口内坐标，然后才交给共用输入连接。普通 Xorg / 独立 Xvfb 继续使用 XTEST。

系统返回的恢复凭据保存在 `~/.local/state/lan-remoteapp/desktop-authorization.json`，文件 0600、目录 0700，不传给控制端、不记录到日志。每次成功恢复立即原子替换一次性凭据。Agent 或 portal 服务重启后，重新接管会自动恢复；操作系统撤销授权、凭据丢失/损坏，或者首次没有允许“记住”时，系统可能要求重新确认。用户取消后不会因继续打字不断弹窗，主动重新连接才再次申请。

这条路径另需系统 Python GI / Gio、`libei.so.1`、RemoteDesktop Portal v2 和 Host Registry。Ubuntu 26.04 / GNOME 50 已具备这些接口；不支持或组件缺失时报告授权连接不可用，不回退到每应用申请。系统弹窗不属于目标应用，不转发到控制端；Mac 窗口副标题会显示授权等待、取消、输入暂停和恢复状态。等待/锁屏期间的点击和文字不缓存补发；断开只释放该控制会话按住的键/按钮，保留共用授权。短暂设备暂停期间需要释放的输入，会在设备恢复时补发释放动作；不会补发按下动作。系统移除设备则丢弃其状态。

## 当前实现边界

- 原生 x11rb 查询 `_NET_CLIENT_LIST` 加 override-redirect 弹出窗口。MIT-SHM 1.2 可用时，通过同一认证连接取得私有 fd，以只读映射接收整窗像素，并复制为独立不可变快照；每个 Backend 的映射上限 32MB。失败或不支持时回到 socket GetImage。现有窗口接入优先读取 XComposite pixmap；不保证透明、形状窗口及复杂菜单完整。采样最短间隔约 16.7ms，目标上限 60fps，实际帧率由窗口、编码与链路决定。编码和写出在独立线程，待发送帧按窗口合并，仅保留最新帧；原始像素相同则跳过编码。原始帧队列及去重缓存各限制在 16M 像素，控制事件最多 256 条。当前仍为软件 H.264 编码。
- XDamage 只订阅已授权的可见窗口，重绘事件通过 X11 fd 唤醒采集，静止窗口跳过读像素；窗口元数据仍每约 16.7ms 检查。不支持 DAMAGE 或窗口订阅失败时保持原轮询路径。采集前重置损坏区域，采集期间的新重绘会再次通知；失败保留重试，缩放或重连强制刷新。请求通道通过非阻塞私有唤醒 socket 与 X11 就绪共同等待，剪贴板传输仍按原顺序处理，不在后台同步剪贴板。
- 新独立会话使用 Xvfb + Openbox + dbus-run-session；已有桌面窗口保留原进程和显示。单会话单控制端。网络断开保留进程；重新附着发送快照。断开释放按键和鼠标按钮，避免修饰键卡住。
- 指针坐标先从目标窗口换算到 X11 根窗口。原桌面 Xwayland 的移动、点击、滚轮和键盘共用 Agent 的 libei 连接；普通 X11 / Xvfb 使用 XTEST。仅移动鼠标不激活应用；已有目标或子窗口焦点时，不在每次键入重复请求窗口管理器激活。
- 状态和幂等请求表目前在内存；Agent 服务重启不保证会话恢复。无持久快照。启动新会话时回收已退出会话；最多 16 个活动会话、4096 个幂等请求记录，超出后需保存工作再重启服务。
- 最多传输 16 个窗口。协议尺寸不超过 4096×4096 且单窗口不超过 8M 像素；当前宿主设置范围为 4096×2160 且不超过 8M。分辨率与 1×/2× 界面缩放分开协商。托管窗口通过 EWMH 解除最大化并请求精确像素宽高；恢复出的超限大窗口会先归一到预算内，避免主窗口消失。
- 输入法在控制端组合，Agent 只接收已提交文字。新建的自有应用实例使用 GTK simple / Qt compose 和 `@im=none`，覆盖服务进程继承的桌面 IM 模块；不改系统输入法配置或现有应用环境。主动复制/粘贴支持双向文本、PNG 截图、普通文件及文件夹；大内容走独立 QUIC 可靠流，详见[剪贴板说明](clipboard.md)。文本每次最多 8192 字节；不做后台同步。X11 selection 交付和 XRes 1.2 PID 校验均由 Agent 完成，不依赖 xclip；文本交付期间保持输入顺序，3 秒超时失败。被控端剪贴板会受本次文字交付或复制操作影响，不读取、保存或恢复其之前的内容。
- 普通原生 X11 应用保留无固定等待的文字路径。Firefox 的异步剪贴板快照需要在新 selection 后留出 16ms，再以每次转换 4ms 的节奏发送复制/粘贴快捷键；Wayland 桥接保留原有 12ms 修饰键节奏和 selection 广告等待。提前读取数据的应用也会被正确计入完成判断，不能因读操作早于快捷键而一直占住输入队列。文字/复制保留应用已聚焦的子窗口，避免重复激活顶层窗口。selection 等待以 X11 fd 可读就绪唤醒。排队中同一窗口的相邻文字提交在 8192 字节内合并；按键、显式粘贴、点击及不同窗口构成顺序屏障，不修改正在交付的文字。
- 未实现多客户端共享控制、可靠复杂菜单/透明窗口、GNOME 50 以外的原生 Wayland 后端、GPU 编码。原生 Wayland 分数缩放、多显示器和真实 Firefox 输入仍需实机验收。自定义快捷键、不采用标准复制/粘贴行为的应用须另行适配；X11 终端使用 Ctrl+Shift+C/V，其他应用使用 Ctrl+C/V。
- 新实例等待可见窗口，十秒内未出现则返回待确认状态并保留会话，提前退出明确报错；实际应用兼容性须实机测试。提供开发安装脚本，无签名/公证或生产 SLA。
- 网络连接断开不等于退出应用；关闭 Mac 宿主窗口也只断开连接；用户操作远端应用画面内的关闭按钮或退出菜单才会结束程序。服务退出会结束其自行启动的子进程，停止服务前应保存工作；显式接入的现有进程不归 Agent 管理，不会被结束。

## 协议

参考 `docs/wire-contract.md`。每条请求必须 `version:1`，第一条 hello。请求限制 64 KiB，事件行限制 16 MiB。PNG 全帧 base64，不接受客户端 shell 字符串。慢接收端写超时后断开，进程仍保留。
