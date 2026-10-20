# Linux Agent 开发版

普通用户运行，不更改当前 GNOME/Wayland。运行依赖：Xvfb、Openbox、xauth、dbus-run-session、已批准应用；P0 还需 Xpra。不会自动安装、启用 linger、改 SSH 配置或防火墙。

## 手工安装

1. 编译：`cargo build --release -p remoteapp-agent`。
2. 将二进制安装到 `~/.local/bin/remoteapp-agent`（SSH 固定命令使用此路径）。
3. 创建 `~/.config/lan-remoteapp/apps.json`，参考 `deploy/apps.example.json`；审核每项绝对路径及 argv。xterm 能执行任何该用户可运行命令，白名单并非 OS 沙箱。
4. 前台运行 `~/.local/bin/remoteapp-agent serve`。或将 service 文件放入 `~/.config/systemd/user/`，由用户自行执行 `systemctl --user daemon-reload` 和 `systemctl --user enable --now remoteapp-agent`。
5. Mac 已经通过 SSH 信任主机并能非交互登录后，启动器可连接。

状态目录 `~/.local/state/lan-remoteapp` 权限 0700；agent.sock 0600。Xvfb 使用共享用户 Xauthority 中每显示独立随机 cookie，关闭 TCP。桥接仅 Unix socket，不额外打开网络服务。SSH 认证仍属于系统；不保存密码和私钥。

## 当前实现边界

- P0 Xpra 原生 seamless 传输；应用通过固定 helper 与白名单启动。剪贴板、文件、打印、音视频默认关闭。
- P1 原生 x11rb 查询 `_NET_CLIENT_LIST` 加 override-redirect 弹出窗口，轮询捕获 GetImage 并编码 PNG（未用 XComposite，窗口遮挡、透明及复杂菜单内容不保证完整）。默认约 10fps 静止帧去重，尚未使用 XDamage/GStreamer/硬编。后台仍会采集/编码静止窗口；不等于低 CPU 的最终版本。
- 每会话 Xvfb + Openbox + dbus-run-session。单会话单控制端。网络断开保留进程；重新附着发送快照。断开释放按键和鼠标按钮，避免修饰键卡住。
- 状态和幂等请求表目前在内存；Agent 服务重启不保证会话恢复。无持久快照。启动新会话时回收已退出会话；最多 16 个活动会话、4096 个幂等请求记录，超出后需保存工作再重启服务。
- 最多传输 16 个窗口。尺寸不超过 4096×4096 且总像素不超过 8M。虚拟屏幕 4096×2160；尚无真正 Retina DPI 重排协商。
- 未实现中文输入法桥接、剪贴板、多客户端共享控制、可靠复杂菜单/透明窗口、Wayland 原生程序、GPU 加速。`input.text.commit` 明确返回 unsupported。
- 窗口管理器 readiness 仅采用短暂启动等待；实际应用兼容性须实机测试。无安装器、签名/公证或生产 SLA。
- SSH 连接断开不等于退出应用；用户关闭应用窗口才会退出程序。服务退出会结束其管理的子进程，停止服务前应保存工作。

## 协议

参考 `docs/wire-contract.md`。每条请求必须 `version:1`，第一条 hello。请求限制 64 KiB，事件行限制 16 MiB。PNG 全帧 base64，不接受客户端 shell 字符串。慢接收端写超时后断开，进程仍保留。
