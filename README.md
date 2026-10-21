# LAN RemoteApp

在 Mac 上启动并操作运行在 Linux 上的图形应用。应用和文件仍然位于 Linux，不是把 Linux 二进制转换为 macOS 程序。

**下一阶段方向（2026-10-22）：** 在保留 Ubuntu → Mac 链路的基础上补齐 Mac → Ubuntu。每个平台交付一个 LAN RemoteApp，统一管理本机服务、应用发布和远端连接；同一台机器可同时发布本机应用、使用其他机器的应用，无需分别安装服务端版和客户端版。架构预留同时连接两台或更多设备的能力，各设备的连接和会话独立管理。当前尚未实现反向链路或完成多设备并发验收，改造边界见[统一节点架构](docs/architecture.md#统一节点架构下一阶段)。

**状态：原生链路开发者预览。** 已在 Apple Silicon Mac 与 Ubuntu 26.04 上验证 xterm 的真实画面、键鼠输入、窗口缩放、隐藏恢复和断开重连。控制端输入法与主动双向文本、截图及文件剪贴板已实现，并在原桌面 Firefox 上实测点击、中文输入、多行粘贴和复制回传。以 `docs/validation.md` 的实际记录为准。

默认自动适应本机窗口：拉大、缩小或全屏时，远端分辨率跟随内容区与本机像素密度调整，画面铺满窗口。“显示 → 自动适应窗口”可随时恢复此模式；也可在“设置分辨率…”中指定固定像素宽高。已用独立 Firefox 实测 2560×1664。界面缩放单独选择 100%/200%，手动固定模式按比例显示并映射鼠标。

## 运行链路

启动器真实读取 Ubuntu 已安装的图形应用名称和图标；“运行中的桌面窗口”和“远程会话”分别显示。应用安装或卸载后重新读取即可更新。原来的 apps.json 作为可选自定义入口保留，详见 [应用与窗口发现](docs/application-discovery.md)。

Linux Agent 在独立 Xvfb + Openbox 会话中运行服务端发现或配置的应用，按窗口采集并默认协商 H.264 实时视频，经严格验证的 SSH 传给 Swift/AppKit `NSWindow`，Mac 使用 VideoToolbox 解码。键鼠、大小和会话控制仍由自有控制协议处理。窗口副标题显示当前使用视频还是 PNG 回退。

默认在独立虚拟桌面启动应用。也可通过服务端白名单显式接入当前桌面中指定进程的 X11/XWayland 窗口，保留原进程与登录状态。不开放公网控制端口。

## 仓库

- `apps/linux-agent`：用户级 Agent、SSH stdio 桥接、持久图形会话。
- `apps/launcher`：Tauri 2 + Vue/TypeScript 设备与应用启动器。
- `apps/mac-windowhost`：Swift/AppKit 原生窗口宿主。
- `crates/`：协议、传输和 Linux 后端等 Rust 组件。
- `deploy/`：人工审核并安装的 Linux 用户服务、白名单示例。
- `docs/design-v1.md`：产品设计与阶段规划。
- `docs/wire-contract.md`：当前可实现的 SSH JSON Lines 契约。
- `docs/architecture.md`：实现与设计差异。
- `docs/threat-model.md`：安全边界。
- `docs/acceptance.md`：实机验收覆盖范围与剩余步骤。

## 使用顺序

1. 阅读 Linux Agent、Mac WindowHost 和 Launcher 目录内 README，按对应平台安装依赖并编译。不要在没有图形运行依赖的 Linux 上将编译成功当作可运行。
2. 默认读取该 Linux 用户已安装的桌面应用；可额外配置 apps.json，保留固定参数或已有进程的自定义入口。客户端只能提交服务端列出的 ID，不能提交任意程序或 shell 命令。
3. 在 Linux 安装用户级 Agent，并启动用户服务。服务仅使用用户私有 Unix socket，不监听公网 TCP。
4. 在 Mac 上通过可信渠道核对 Ubuntu SSH 主机指纹。启动器支持账号密码连接，由 Mac 本机后端保存连接配置，网络恢复后自动重新认证与恢复原窗口；不新增 Ubuntu 公钥授权，严格校验主机身份。已有 SSH 密钥/agent 与维护用临时登录辅助脚本仍可使用。
5. 启动 Launcher，填写主机、账号、密码和端口后点“连接”，点击应用即可在原生窗口中启动。已保存密码时可留空，下次打开自动连接。
6. 点击 Mac 窗口红色关闭按钮或退出宿主会断开连接并保留远端应用；需要关闭远端程序时，使用其画面内的关闭按钮或退出菜单。原生模式请保留会话 ID，以便再次连接。

## 当前边界

P1 使用 SSH + 版本化 JSON Lines，默认 H.264 关键帧/P 帧视频；Linux 编码组件缺失、编码失败或旧客户端会使用 PNG 回退。视频采用有损 4:2:0 编码，未实现 WebRTC/UDP、QUIC、Protobuf、Metal 或 XDamage。文字在控制端组合后提交，文本、图片与文件复制/粘贴仅由主动操作触发，不做后台同步；格式、大小和缓存限制见[剪贴板说明](docs/clipboard.md)。精确输入法候选框位置、Retina 双屏、复杂菜单、GPU 应用、断网恢复矩阵与长期性能仍待完整验收。

不要传输密码、私钥、令牌到聊天或仓库。无需管理员权限运行 Agent；软件拥有运行它的 Linux 用户权限，应用白名单不是 OS 沙箱。建议为远程应用使用专用低权限账户。

## 构建与验证

见 `docs/validation.md`。`scripts/check.sh` 用于已装好工具链的开发机器；它不安装依赖、不连接远端、不修改系统服务。Mac 的 Swift/AppKit 构建只能在具备 macOS SDK 的 Mac 完成。
