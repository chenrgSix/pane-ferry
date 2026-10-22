# 窗渡 PaneFerry 命名与升级兼容

2026-10-22 起，产品统一使用 **窗渡 PaneFerry**。界面标题、菜单、权限提示和安装入口使用完整名称；代码模块、命令和构建产物使用 `PaneFerry` / `paneferry`。

| 用途 | 当前名称 |
| --- | --- |
| macOS 主应用 | `窗渡 PaneFerry.app` |
| Linux 管理入口 | `paneferry`（安装到 `~/.local/bin/paneferry`） |
| 启动器 npm / Cargo 包 | `paneferry-launcher` |
| Linux 发布器与公共网络组件 | `paneferry-agent`、`paneferry-network` |
| Rust 库 | `paneferry-protocol`、`paneferry-core`、`paneferry-capture`、`paneferry-backend-x11`、`paneferry-transport` |
| macOS 辅助组件与 Swift target | `PaneFerryNode`、`PaneFerryWindowHost` |
| Linux 默认安装包 | `dist/paneferry-linux.tar.gz` |
| 源码归档 | `paneferry-source.zip`，归档顶层目录 `paneferry/` |

## 保留的兼容标识

以下标识用于已有安装、权限、持久数据或通信兼容，继续保持原值，不应按产品名称批量替换：

- macOS bundle ID、Linux application / desktop ID 与发布过滤规则中的 `io.lanremoteapp.*`、`local.lanremoteapp.*`，避免把升级识别为另一款应用。
- `~/.config/lan-remoteapp`、`~/.local/state/lan-remoteapp`、`~/.cache/lan-remoteapp`、Snap 的 `common/lan-remoteapp`，以及 macOS 的 `Application Support/io.lanremoteapp.launcher` 和 `Caches/LAN RemoteApp/Clipboard`。设备证书、配对、会话、授权恢复凭据和缓存继续从原目录读取。
- 浏览器存储的 `lan-remoteapp.*` 键，保留已选设备、应用目录和待确认的启动请求。
- systemd unit `remoteapp-agent.service`。其显示名称已更新，`ExecStart` 指向 `~/.local/bin/paneferry-agent`；安装脚本继续更新同一个服务，不创建第二个发布器。运维命令仍为 `systemctl --user status remoteapp-agent.service`。
- QUIC ALPN `lan-remoteapp/2`、证书与 SNI 名称 `lan-remoteapp`、RWP/1、RWN/2、预留 Protobuf 命名空间 `remoteapp.v1`、X11 atom `_LAN_REMOTEAPP_*` 和桌面入口属性 `X-LANRemoteApp-SessionBus`。

Linux 程序资源的新安装位置为 `~/.local/share/paneferry`；桌面入口 ID 保持不变，安装后显示新名称并运行新命令。源代码目录所在的本机 checkout 不自动搬迁。

历史验收记录、性能报告和原始日志中的旧产品名、产物路径与哈希按当时实测保留。设计文档中的 Microsoft RemoteApp/RAIL、RemoteApp/RDS 是第三方技术名称，也保留原文。

本次源码改名不自动替换正在运行的应用或重启已安装服务。开发包的可执行文件和路径已变化，系统是否需要重新确认权限，以新版实际运行时的提示为准。
