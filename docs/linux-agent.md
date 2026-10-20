# Linux Agent 开发版

普通用户运行，不更改当前 GNOME/Wayland。运行依赖：Xvfb、Openbox、xauth、dbus-run-session、已批准应用。不会自动安装、启用 linger、改 SSH 配置或防火墙。

## 手工安装

1. 编译：`cargo build --release -p remoteapp-agent`。
2. 将二进制安装到 `~/.local/bin/remoteapp-agent`（SSH 固定命令使用此路径）。
3. 创建 `~/.config/lan-remoteapp/apps.json`，参考 `deploy/apps.example.json`；审核每项绝对路径及 argv。xterm 能执行任何该用户可运行命令，白名单并非 OS 沙箱。
4. 前台运行 `~/.local/bin/remoteapp-agent serve`。或将 service 文件放入 `~/.config/systemd/user/`，由用户自行执行 `systemctl --user daemon-reload` 和 `systemctl --user enable --now remoteapp-agent`。
5. Mac 已经通过 SSH 信任主机并能非交互登录后，启动器可连接。

状态目录 `~/.local/state/lan-remoteapp` 权限 0700；agent.sock 0600。Xvfb 使用共享用户 Xauthority 中每显示独立随机 cookie，关闭 TCP。桥接仅 Unix socket，不额外打开网络服务。SSH 认证仍属于系统；不保存密码和私钥。

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

Xwayland 的模拟键鼠可能触发 Ubuntu 原桌面的远程输入授权弹窗，需要用户在系统弹窗允许。系统弹窗不属于目标应用，当前不会转发到 Mac；仅能看到画面不代表键鼠已获授权。原桌面锁屏也会阻止输入，不能据此判断应用已接收文本。项目不自动授权或修改桌面安全设置。

## 当前实现边界

- 原生 x11rb 查询 `_NET_CLIENT_LIST` 加 override-redirect 弹出窗口，轮询捕获 GetImage 并编码 PNG。现有窗口接入优先读取 XComposite pixmap；不保证透明、形状窗口及复杂菜单完整。默认约 10fps 静止帧去重，尚未使用 XDamage/GStreamer/硬编。后台仍会采集/编码静止窗口；不等于低 CPU 的最终版本。
- 每会话 Xvfb + Openbox + dbus-run-session。单会话单控制端。网络断开保留进程；重新附着发送快照。断开释放按键和鼠标按钮，避免修饰键卡住。
- 状态和幂等请求表目前在内存；Agent 服务重启不保证会话恢复。无持久快照。启动新会话时回收已退出会话；最多 16 个活动会话、4096 个幂等请求记录，超出后需保存工作再重启服务。
- 最多传输 16 个窗口。尺寸不超过 4096×4096 且总像素不超过 8M。虚拟屏幕 4096×2160；尚无真正 Retina DPI 重排协商。
- 输入法在控制端组合，Agent 只接收已提交文字。主动复制/粘贴支持双向 UTF-8 纯文本，每次最多 8192 字节；不做后台同步。X11 selection 交付和 XRes 1.2 PID 校验均由 Agent 完成，不依赖 xclip；文本交付期间保持输入顺序，3 秒超时失败。被控端剪贴板会受本次文字交付或复制操作影响，不读取、保存或恢复其之前的内容。
- 未实现多客户端共享控制、可靠复杂菜单/透明窗口、Wayland 原生程序、GPU 加速。自定义快捷键、不采用标准复制/粘贴行为的应用须另行适配；终端使用 Ctrl+Shift+C/V，其他应用使用 Ctrl+C/V。
- 窗口管理器 readiness 仅采用短暂启动等待；实际应用兼容性须实机测试。无安装器、签名/公证或生产 SLA。
- SSH 连接断开不等于退出应用；用户关闭应用窗口才会请求退出程序。服务退出会结束其自行启动的子进程，停止服务前应保存工作；显式接入的现有进程不归 Agent 管理，不会被结束。

## 协议

参考 `docs/wire-contract.md`。每条请求必须 `version:1`，第一条 hello。请求限制 64 KiB，事件行限制 16 MiB。PNG 全帧 base64，不接受客户端 shell 字符串。慢接收端写超时后断开，进程仍保留。
