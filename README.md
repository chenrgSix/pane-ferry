# 窗渡 PaneFerry

在 Mac 和 Ubuntu 之间双向使用对方的图形应用。每台设备同时可以发布和接收，应用和文件仍留在原设备。

产品显示名称统一为 **窗渡 PaneFerry**，命令与软件包使用 `paneferry` 前缀。升级继续使用原有设备身份、配对和配置；保留的内部标识与新旧名称对应见[命名与升级兼容](docs/naming.md)。

**双向组件（2026-10-22）：** 已加入 Mac 发布、Ubuntu 接收和统一管理入口，每个平台交付一个 窗渡 PaneFerry。设备连接、目录和窗口保留来源设备，预留多设备扩展；实际多设备并发尚未验收。Mac → Ubuntu 已通过真实 H.264 画面、点击、中文与 emoji 提交、双向文本剪贴板、缩放、同会话重连和断开保留应用验证。使用步骤与当前边界见 [Mac 发布指南](docs/mac-publishing.md)。

**原生传输（2026-10-22）：** 两端已接入统一传输、QUIC 可靠流及设备配对，产品仅使用 QUIC。使用方法见[统一传输指南](docs/native-transport.md)；删除 SSH 前后的实机双向回归、多设备隔离和新包范围见[QUIC 独立链路验收](docs/quic-only-validation.md)。

**状态：原生链路开发者预览。** 已在 Apple Silicon Mac 与 Ubuntu 26.04 上验证 xterm 的真实画面、键鼠输入、窗口缩放、隐藏恢复和断开重连。控制端输入法与主动双向文本、截图及文件剪贴板已实现，并在原桌面 Firefox 上实测点击、中文输入、多行粘贴和复制回传。以 `docs/validation.md` 的实际记录为准。

默认自动适应本机窗口：拉大、缩小或全屏时，远端分辨率跟随内容区与本机像素密度调整，画面铺满窗口。“显示 → 自动适应窗口”可随时恢复此模式；也可在“设置分辨率…”中指定固定像素宽高。已用独立 Firefox 实测 2560×1664。界面缩放单独选择 100%/200%，手动固定模式按比例显示并映射鼠标。

## 运行链路

启动器真实读取 Ubuntu 已安装的图形应用名称和图标；“运行中的桌面窗口”和“远程会话”分别显示。应用安装或卸载后重新读取即可更新。原来的 apps.json 作为可选自定义入口保留，详见 [应用与窗口发现](docs/application-discovery.md)。

Linux Agent 在独立 Xvfb + Openbox 会话中运行服务端发现或配置的应用，按窗口采集并默认协商 H.264 实时视频，经已配对 QUIC 传给 Swift/AppKit `NSWindow`，Mac 使用 VideoToolbox 解码。键鼠、大小和会话控制仍由自有控制协议处理。窗口副标题显示当前使用视频还是 PNG 回退。

默认在独立虚拟桌面启动应用。也可通过服务端白名单显式接入当前桌面中指定进程的 X11/XWayland 窗口，保留原进程与登录状态。原生连接默认监听 UDP 42424，面向局域网；不自动配置公网映射。

## 仓库

- `apps/linux-agent`：用户级发布器、私有本机接口、持久图形会话。
- `apps/launcher`：Tauri 2 + Vue/TypeScript 设备与应用启动器。
- `apps/mac-windowhost`：Swift/AppKit 原生窗口宿主。
- `apps/mac-node`：Mac 本机窗口发布、H.264 编码和输入执行辅助组件。
- `apps/linux-client`：同一个 Ubuntu 产品中的 GTK 管理界面、输入法和窗口接收组件。
- `crates/network`：两端共用的 QUIC、配对、设备路由和二进制流组件。
- `crates/`：协议、传输和 Linux 后端等 Rust 组件。
- `deploy/`：人工审核并安装的 Linux 用户服务、白名单示例。
- `docs/design-v1.md`：产品设计与阶段规划。
- `docs/wire-contract.md`：当前 RWP/1 本机业务契约。
- `docs/architecture.md`：实现与设计差异。
- `docs/threat-model.md`：安全边界。
- `docs/acceptance.md`：实机验收覆盖范围与剩余步骤。

## 使用顺序

1. 阅读 Linux Agent、Mac WindowHost 和 Launcher 目录内 README，按对应平台安装依赖并编译。不要在没有图形运行依赖的 Linux 上将编译成功当作可运行。
2. 默认读取该 Linux 用户已安装的桌面应用；可额外配置 apps.json，保留固定参数或已有进程的自定义入口。客户端只能提交服务端列出的 ID，不能提交任意程序或 shell 命令。
3. 在 Linux 安装用户级 Agent，并启动用户服务。发布器使用用户私有 Unix socket；打开管理界面后启动 QUIC 设备连接组件。
4. 按[统一传输指南](docs/native-transport.md)在任一端生成邀请，另一端填写对方地址并配对。一次配对支持双向使用，无需 SSH 账号、密码或返回通道。
5. 在管理界面选择已配对设备、读取应用并打开原生窗口。可为多台设备分别配对；切换列表不会改变已打开窗口的归属。
6. 点击 Mac 窗口红色关闭按钮或退出宿主会断开连接并保留远端应用；需要关闭远端程序时，使用其画面内的关闭按钮或退出菜单。原生模式请保留会话 ID，以便再次连接。

## 当前边界

本机兼容接口使用版本化 JSON Lines，网络使用 QUIC 可靠流；默认 H.264 关键帧/P 帧视频；Linux 编码组件缺失、编码失败或旧客户端会使用 PNG 回退。视频采用有损 4:2:0 编码，已实现基于 UDP 的 QUIC 可靠流与 XDamage；未实现 WebRTC、QUIC DATAGRAM、Protobuf 或 Metal。文字在控制端组合后提交，文本、图片与文件复制/粘贴仅由主动操作触发，不做后台同步；格式、大小和缓存限制见[剪贴板说明](docs/clipboard.md)。精确输入法候选框位置、Retina 双屏、复杂菜单、GPU 应用、断网恢复矩阵与长期性能仍待完整验收。

不要传输密码、私钥、令牌到聊天或仓库。无需管理员权限运行 Agent；软件拥有运行它的 Linux 用户权限，应用白名单不是 OS 沙箱。建议为远程应用使用专用低权限账户。

## 构建与验证

见 `docs/validation.md`。`scripts/check.sh` 用于已装好工具链的开发机器；它不安装依赖、不连接远端、不修改系统服务。Mac 的 Swift/AppKit 构建只能在具备 macOS SDK 的 Mac 完成。
