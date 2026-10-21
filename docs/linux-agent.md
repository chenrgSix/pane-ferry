# Linux Agent 开发版

普通用户运行，不更改当前 GNOME/Wayland。运行依赖：Xvfb、Openbox、xauth、dbus-run-session；应用发现另使用系统 Python GI、GIO/GioUnix、GTK3 和 GdkPixbuf。不会自动安装、启用 linger、改 SSH 配置或防火墙。

## 手工安装

1. 编译：`cargo build --release -p remoteapp-agent`。
2. 将二进制安装到 `~/.local/bin/remoteapp-agent`（SSH 固定命令使用此路径）。
3. 默认发现当前用户已安装的桌面应用。可选的 `~/.config/lan-remoteapp/apps.json` 用于自定义入口，格式参考 `deploy/apps.example.json`；审核其中的绝对路径及 argv。没有此文件也能运行。应用仍具有当前用户权限，目录不是 OS 沙箱。
4. 前台运行 `~/.local/bin/remoteapp-agent serve`。或将 service 文件放入 `~/.config/systemd/user/`，由用户自行执行 `systemctl --user daemon-reload` 和 `systemctl --user enable --now remoteapp-agent`。
5. Mac 已经通过 SSH 信任主机并能非交互登录后，启动器可连接。

状态目录 `~/.local/state/lan-remoteapp` 权限 0700；agent.sock 0600。Xvfb 使用共享用户 Xauthority 中每显示独立随机 cookie，关闭 TCP。桥接仅 Unix socket，不额外打开网络服务。SSH 认证仍属于系统；不保存密码和私钥。

## 窗口视频流

新版 Mac 宿主优先协商 H.264。Agent 的独立输出线程运行系统 GStreamer 的 appsrc、videoconvert、openh264enc、h264parse、appsink 管线，奇数宽高才插入 videobox 补边。通过稳定 C API 运行时加载 `libgstreamer-1.0.so.0` / `libgstapp-1.0.so.0`，视频编码不再依赖 Python GI 或原始帧 socketpair。只读像素缓冲由 Arc 租约保持至 GStreamer 释放，避免跨进程传帧、Python 缓冲拼接和再次填充 GstBuffer。identity 只按原 sequence/30 规则设置时间戳；颜色转换及编码器、码率、GOP、QP 上限、帧跳过策略保持不变。拉取编码结果有 2 秒超时，失败维持现有 PNG 回退；停止管线后释放缓冲及对象引用。不运行客户端提供的管线或命令。

缓冲共享遵循 [GStreamer 的 wrapped-buffer 生命周期约定](https://gstreamer.freedesktop.org/documentation/gstreamer/gstbuffer.html#gst_buffer_new_wrapped_full)：数据保持只读，释放通知回收租约，不依赖私有结构体布局。`tests/performance/legacy_video_encoder.py` 仅用于与旧管线比较解码像素，不再作为运行时编码进程。

编码使用 OpenH264 屏幕模式、无 B 帧、关键帧加 P 帧；窗口改变大小、隐藏恢复或重连后从新的关键帧开始。仅在编码之前合并旧画面，已编码的依赖帧不丢弃。实际显示像素尺寸保持不变，奇数宽高为编码补至偶数，Mac 解码后裁掉补边。编码为有损 4:2:0，文本颜色精度与 PNG 不同。

Agent 在 hello 中仅在组件可用时声明 `native.h264`；没有组件或编码失败时保留 PNG，Mac 副标题明确显示回退。当前仍经 SSH 传输，不是 WebRTC/UDP；不会配置 STUN、TURN、公网端口或更改防火墙。Ubuntu 已有组件在实机测试中可用，但其他 Linux 发行版需要自行安装运行依赖。

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

`existing.scale_factor` 可选，默认为 1，仅用于声明应用已经采用的 1×/2× 缩放；Agent 不更改原桌面的全局 DPI。新独立会话的 `app.launch.scale_factor` 会配置 GTK/Qt 缩放；GTK 已实测，Qt 和传统 Xlib 应用仍需逐项验证。不支持界面缩放的应用可在启动器选择 100%。

Firefox Snap 可选用 `deploy/firefox-x11.sh` 作为经过审核的固定 executable：安装到用户目录，并为独立测试资料指定固定 argv。脚本仅抽取本显示号的 cookie 到 Snap 可读的私有文件，保持 0600 权限；在临时用户 scope 前台运行 Firefox，以满足 Snap 的 cgroup 要求，不关闭其沙箱或认证。退出 Agent 时仍通过原进程组清理自己启动的程序。

### 原桌面统一输入授权

Ubuntu Wayland / Xwayland 原桌面的键鼠由 **LAN RemoteApp Agent** 统一申请和管理。首次接管时，在 Ubuntu 的系统窗口确认控制授权并保留“记住”选项；授权的是同一 Ubuntu 用户的 Agent。之后新增应用、关闭控制端窗口、SSH 断线和重新连接，都复用 Agent 的同一个输入会话，不按应用另弹分享窗口。应用列表发现、只准备会话（`attach:false`）和独立 Xvfb 应用不申请原桌面授权。

Agent 使用 RemoteDesktop Portal v2 的 `persist_mode=2` 与系统 libei，申请键盘和指针；不申请 ScreenCast 或系统剪贴板，不扩大全部窗口的捕获范围。原有 XComposite 采集、H.264 over SSH 和自有双向剪贴板通道保持原样。所有输入仍先核验所选窗口、进程身份和窗口内坐标，然后才交给共用输入连接。普通 Xorg / 独立 Xvfb 继续使用 XTEST。

系统返回的恢复凭据保存在 `~/.local/state/lan-remoteapp/desktop-authorization.json`，文件 0600、目录 0700，不传给控制端、不记录到日志。每次成功恢复立即原子替换一次性凭据。Agent 或 portal 服务重启后，重新接管会自动恢复；操作系统撤销授权、凭据丢失/损坏，或者首次没有允许“记住”时，系统可能要求重新确认。用户取消后不会因继续打字不断弹窗，主动重新连接才再次申请。

这条路径另需系统 Python GI / Gio、`libei.so.1`、RemoteDesktop Portal v2 和 Host Registry。Ubuntu 26.04 / GNOME 50 已具备这些接口；不支持或组件缺失时报告授权连接不可用，不回退到每应用申请。系统弹窗不属于目标应用，不转发到控制端；Mac 窗口副标题会显示授权等待、取消、输入暂停和恢复状态。等待/锁屏期间的点击和文字不缓存补发；断开只释放该控制会话按住的键/按钮，保留共用授权。短暂设备暂停期间需要释放的输入，会在设备恢复时补发释放动作；不会补发按下动作。系统移除设备则丢弃其状态。

## 当前实现边界

- 原生 x11rb 查询 `_NET_CLIENT_LIST` 加 override-redirect 弹出窗口。MIT-SHM 1.2 可用时，通过同一认证连接取得私有 fd，以只读映射接收整窗像素，并复制为独立不可变快照；每个 Backend 的映射上限 32MB。失败或不支持时回到 socket GetImage。现有窗口接入优先读取 XComposite pixmap；不保证透明、形状窗口及复杂菜单完整。采样最短间隔 33ms，上限约 30fps，实际帧率由窗口、编码与链路决定。编码和写出在独立线程，待发送帧按窗口合并，仅保留最新帧；原始像素相同则跳过编码。原始帧队列及去重缓存各限制在 16M 像素，控制事件最多 256 条。当前仍为软件 H.264 编码。
- XDamage 只订阅已授权的可见窗口，重绘事件通过 X11 fd 唤醒采集，静止窗口跳过读像素；窗口元数据仍每约 33ms 检查。不支持 DAMAGE 或窗口订阅失败时保持原轮询路径。采集前重置损坏区域，采集期间的新重绘会再次通知；失败保留重试，缩放或重连强制刷新。请求通道通过非阻塞私有唤醒 socket 与 X11 就绪共同等待，剪贴板传输仍按原顺序处理，不在后台同步剪贴板。
- 每会话 Xvfb + Openbox + dbus-run-session。单会话单控制端。网络断开保留进程；重新附着发送快照。断开释放按键和鼠标按钮，避免修饰键卡住。
- 指针坐标先从目标窗口换算到 X11 根窗口。原桌面 Xwayland 的移动、点击、滚轮和键盘共用 Agent 的 libei 连接；普通 X11 / Xvfb 使用 XTEST。仅移动鼠标不激活应用；已有目标或子窗口焦点时，不在每次键入重复请求窗口管理器激活。
- 状态和幂等请求表目前在内存；Agent 服务重启不保证会话恢复。无持久快照。启动新会话时回收已退出会话；最多 16 个活动会话、4096 个幂等请求记录，超出后需保存工作再重启服务。
- 最多传输 16 个窗口。协议尺寸不超过 4096×4096 且单窗口不超过 8M 像素；当前宿主设置范围为 4096×2160 且不超过 8M。分辨率与 1×/2× 界面缩放分开协商。托管窗口通过 EWMH 解除最大化并请求精确像素宽高；恢复出的超限大窗口会先归一到预算内，避免主窗口消失。
- 输入法在控制端组合，Agent 只接收已提交文字。新建的自有应用实例使用 GTK simple / Qt compose 和 `@im=none`，覆盖服务进程继承的桌面 IM 模块；不改系统输入法配置或现有应用环境。主动复制/粘贴支持双向文本、PNG 截图、普通文件及文件夹；大内容走自有 SSH 字节流，详见[剪贴板说明](clipboard.md)。文本每次最多 8192 字节；不做后台同步。X11 selection 交付和 XRes 1.2 PID 校验均由 Agent 完成，不依赖 xclip；文本交付期间保持输入顺序，3 秒超时失败。被控端剪贴板会受本次文字交付或复制操作影响，不读取、保存或恢复其之前的内容。
- 普通原生 X11 应用保留无固定等待的文字路径。Firefox 的异步剪贴板快照需要在新 selection 后留出 16ms，再以每次转换 4ms 的节奏发送复制/粘贴快捷键；Wayland 桥接保留原有 12ms 修饰键节奏和 selection 广告等待。提前读取数据的应用也会被正确计入完成判断，不能因读操作早于快捷键而一直占住输入队列。文字/复制保留应用已聚焦的子窗口，避免重复激活顶层窗口。selection 等待以 X11 fd 可读就绪唤醒。排队中同一窗口的相邻文字提交在 8192 字节内合并；按键、显式粘贴、点击及不同窗口构成顺序屏障，不修改正在交付的文字。
- 未实现多客户端共享控制、可靠复杂菜单/透明窗口、Wayland 原生程序、GPU 加速。自定义快捷键、不采用标准复制/粘贴行为的应用须另行适配；终端使用 Ctrl+Shift+C/V，其他应用使用 Ctrl+C/V。
- 窗口管理器 readiness 仅采用短暂启动等待；实际应用兼容性须实机测试。无安装器、签名/公证或生产 SLA。
- SSH 连接断开不等于退出应用；关闭 Mac 宿主窗口也只断开连接；用户操作远端应用画面内的关闭按钮或退出菜单才会结束程序。服务退出会结束其自行启动的子进程，停止服务前应保存工作；显式接入的现有进程不归 Agent 管理，不会被结束。

## 协议

参考 `docs/wire-contract.md`。每条请求必须 `version:1`，第一条 hello。请求限制 64 KiB，事件行限制 16 MiB。PNG 全帧 base64，不接受客户端 shell 字符串。慢接收端写超时后断开，进程仍保留。
