# Linux Agent 开发版

普通用户运行，不更改当前 GNOME/Wayland。运行依赖：Xvfb、Openbox、xauth、dbus-run-session、已批准应用。不会自动安装、启用 linger、改 SSH 配置或防火墙。

## 手工安装

1. 编译：`cargo build --release -p remoteapp-agent`。
2. 将二进制安装到 `~/.local/bin/remoteapp-agent`（SSH 固定命令使用此路径）。
3. 创建 `~/.config/lan-remoteapp/apps.json`，参考 `deploy/apps.example.json`；审核每项绝对路径及 argv。xterm 能执行任何该用户可运行命令，白名单并非 OS 沙箱。
4. 前台运行 `~/.local/bin/remoteapp-agent serve`。或将 service 文件放入 `~/.config/systemd/user/`，由用户自行执行 `systemctl --user daemon-reload` 和 `systemctl --user enable --now remoteapp-agent`。
5. Mac 已经通过 SSH 信任主机并能非交互登录后，启动器可连接。

状态目录 `~/.local/state/lan-remoteapp` 权限 0700；agent.sock 0600。Xvfb 使用共享用户 Xauthority 中每显示独立随机 cookie，关闭 TCP。桥接仅 Unix socket，不额外打开网络服务。SSH 认证仍属于系统；不保存密码和私钥。

## 窗口视频流

新版 Mac 宿主优先协商 H.264。Agent 将自己已验证归属的窗口原始像素交给固定的私有编码进程，使用系统 `/usr/bin/python3`、PyGObject/GstApp 和 GStreamer 的 appsrc、videobox、videoconvert、openh264enc、h264parse、appsink。辅助源码嵌入二进制，不额外部署脚本，不运行客户端提供的命令。输入输出使用私有 socketpair，有 3 秒 I/O 超时；会话和图片输出线程独立处理，编码进程在流结束时回收。

编码使用 OpenH264 屏幕模式、无 B 帧、关键帧加 P 帧；窗口改变大小、隐藏恢复或重连后从新的关键帧开始。仅在编码之前合并旧画面，已编码的依赖帧不丢弃。实际显示像素尺寸保持不变，奇数宽高为编码补至偶数，Mac 解码后裁掉补边。编码为有损 4:2:0，文本颜色精度与 PNG 不同。

Agent 在 hello 中仅在组件可用时声明 `native.h264`；没有组件或编码失败时保留 PNG，Mac 副标题明确显示回退。当前仍经 SSH 传输，不是 WebRTC/UDP；不会配置 STUN、TURN、公网端口或更改防火墙。Ubuntu 已有组件在实机测试中可用，但其他 Linux 发行版需要自行安装运行依赖。

## 接入现有 X11 窗口

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

同一进程的重复连接请求复用会话，并保持单控制端限制。断开或停止 Agent 不结束此类原有进程；关闭 Mac 窗口仍会向原窗口发送关闭请求。原进程退出后需要在本地重新核对并更新 PID。原生 Wayland 窗口不支持此方式；现有桌面上的尺寸和输入会作用于原窗口。

应用目录会标明已退出或不匹配的现有进程，启动器禁用该入口并解释原因；不会自动接管同名的新进程。请求期间进程退出会返回 `existing_process_unavailable`。已结束/回收的会话分别返回 `session_exited` / `session_expired`，启动器在明确拒绝后清除旧启动请求 ID；网络超时和未知错误仍保留 ID，避免重试重复启动。

`existing.scale_factor` 可选，默认为 1，仅用于声明应用已经采用的 1×/2× 缩放；Agent 不更改原桌面的全局 DPI。新独立会话的 `app.launch.scale_factor` 会配置 GTK/Qt 缩放；GTK 已实测，Qt 和传统 Xlib 应用仍需逐项验证。不支持界面缩放的应用可在启动器选择 100%。

Firefox Snap 可选用 `deploy/firefox-x11.sh` 作为经过审核的固定 executable：安装到用户目录，并为独立测试资料指定固定 argv。脚本仅抽取本显示号的 cookie 到 Snap 可读的私有文件，保持 0600 权限；在临时用户 scope 前台运行 Firefox，以满足 Snap 的 cgroup 要求，不关闭其沙箱或认证。退出 Agent 时仍通过原进程组清理自己启动的程序。

Xwayland 的模拟键鼠可能触发 Ubuntu 原桌面的远程输入授权弹窗，需要用户在系统弹窗允许。系统弹窗不属于目标应用，当前不会转发到 Mac；仅能看到画面不代表键鼠已获授权。原桌面锁屏也会阻止输入，不能据此判断应用已接收文本。项目不自动授权或修改桌面安全设置。

## 当前实现边界

- 原生 x11rb 查询 `_NET_CLIENT_LIST` 加 override-redirect 弹出窗口，轮询捕获 GetImage。现有窗口接入优先读取 XComposite pixmap；不保证透明、形状窗口及复杂菜单完整。采样最短间隔 33ms，上限约 30fps，实际帧率由窗口、编码与链路决定。PNG 编码和写出在独立线程，待发送帧按窗口合并，仅保留最新帧；静止原始像素相同则跳过编码。原始帧队列及去重缓存各限制在 16M 像素，控制事件最多 256 条。尚未使用 XDamage/GStreamer/硬编，静止窗口仍轮询取图，不等于低 CPU 的最终版本。
- 每会话 Xvfb + Openbox + dbus-run-session。单会话单控制端。网络断开保留进程；重新附着发送快照。断开释放按键和鼠标按钮，避免修饰键卡住。
- 指针坐标先从目标窗口换算到 X11 根窗口，移动和点击统一使用 XTEST，以适配 Xwayland 的输入门户。仅移动鼠标不激活应用；已有目标或子窗口焦点时，不在每次键入重复请求窗口管理器激活。
- 状态和幂等请求表目前在内存；Agent 服务重启不保证会话恢复。无持久快照。启动新会话时回收已退出会话；最多 16 个活动会话、4096 个幂等请求记录，超出后需保存工作再重启服务。
- 最多传输 16 个窗口。协议尺寸不超过 4096×4096 且单窗口不超过 8M 像素；当前宿主设置范围为 4096×2160 且不超过 8M。分辨率与 1×/2× 界面缩放分开协商。托管窗口通过 EWMH 解除最大化并请求精确像素宽高；恢复出的超限大窗口会先归一到预算内，避免主窗口消失。
- 输入法在控制端组合，Agent 只接收已提交文字。主动复制/粘贴支持双向 UTF-8 纯文本，每次最多 8192 字节；不做后台同步。X11 selection 交付和 XRes 1.2 PID 校验均由 Agent 完成，不依赖 xclip；文本交付期间保持输入顺序，3 秒超时失败。被控端剪贴板会受本次文字交付或复制操作影响，不读取、保存或恢复其之前的内容。
- 未实现多客户端共享控制、可靠复杂菜单/透明窗口、Wayland 原生程序、GPU 加速。自定义快捷键、不采用标准复制/粘贴行为的应用须另行适配；终端使用 Ctrl+Shift+C/V，其他应用使用 Ctrl+C/V。
- 窗口管理器 readiness 仅采用短暂启动等待；实际应用兼容性须实机测试。无安装器、签名/公证或生产 SLA。
- SSH 连接断开不等于退出应用；用户关闭应用窗口才会请求退出程序。服务退出会结束其自行启动的子进程，停止服务前应保存工作；显式接入的现有进程不归 Agent 管理，不会被结束。

## 协议

参考 `docs/wire-contract.md`。每条请求必须 `version:1`，第一条 hello。请求限制 64 KiB，事件行限制 16 MiB。PNG 全帧 base64，不接受客户端 shell 字符串。慢接收端写超时后断开，进程仍保留。
