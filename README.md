# LAN RemoteApp

在 Mac 上启动并操作运行在 Linux 上的图形应用。应用和文件仍然位于 Linux，不是把 Linux 二进制转换为 macOS 程序。

**状态：原生链路开发者预览。** 已在 Apple Silicon Mac 与 Ubuntu 26.04 上验证 xterm 的真实画面、键鼠输入、窗口缩放、隐藏恢复和断开重连。控制端输入法与主动双向纯文本剪贴板已实现，并在原桌面 Firefox 上实测点击、中文输入、多行粘贴和复制回传。以 `docs/validation.md` 的实际记录为准。

默认自动适应本机窗口：拉大、缩小或全屏时，远端分辨率跟随内容区与本机像素密度调整，画面铺满窗口。“显示 → 自动适应窗口”可随时恢复此模式；也可在“设置分辨率…”中指定固定像素宽高。已用独立 Firefox 实测 2560×1664。界面缩放单独选择 100%/200%，手动固定模式按比例显示并映射鼠标。

## 运行链路

Linux Agent 在独立 Xvfb + Openbox 会话中运行白名单应用，按窗口采集并默认协商 H.264 实时视频，经严格验证的 SSH 传给 Swift/AppKit `NSWindow`，Mac 使用 VideoToolbox 解码。键鼠、大小和关闭请求仍由自有控制协议处理。窗口副标题显示当前使用视频还是 PNG 回退。

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
2. 在 Linux 本地审核应用白名单配置，只批准固定的绝对可执行文件和固定参数。客户端不能提交任意程序或 shell 命令。
3. 在 Linux 安装用户级 Agent，并启动用户服务。服务仅使用用户私有 Unix socket，不监听公网 TCP。
4. 在 Mac 上先通过可信渠道核对 Ubuntu SSH 主机指纹，并手工建立正常 SSH 登录。密码用户可用 `bash scripts/connect-ssh.sh HOST USER 22` 建立临时登录会话，密码由系统 SSH 处理。程序使用 BatchMode 和严格主机密钥检查，不自动接受未知或变化的主机密钥，不提供密码收集界面。
5. 启动 Launcher，填写已验证主机、SSH 用户和端口，加载批准应用目录，点击应用即可在原生窗口中启动。
6. 断开连接与关闭远端应用不同：关闭窗口会请求远端应用退出；退出/断开客户端不应删除远端工作。原生模式请保留会话 ID，以便再次连接。

## 当前边界

P1 使用 SSH + 版本化 JSON Lines，默认 H.264 关键帧/P 帧视频；Linux 编码组件缺失、编码失败或旧客户端会使用 PNG 回退。视频采用有损 4:2:0 编码，未实现 WebRTC/UDP、QUIC、Protobuf、Metal 或 XDamage。文字在控制端组合后提交，复制/粘贴仅由主动操作触发，每次最多 8192 字节，不做后台同步。精确输入法候选框位置、Retina 双屏、复杂菜单、GPU 应用、断网恢复矩阵与长期性能仍待完整验收。

不要传输密码、私钥、令牌到聊天或仓库。无需管理员权限运行 Agent；软件拥有运行它的 Linux 用户权限，应用白名单不是 OS 沙箱。建议为远程应用使用专用低权限账户。

## 构建与验证

见 `docs/validation.md`。`scripts/check.sh` 用于已装好工具链的开发机器；它不安装依赖、不连接远端、不修改系统服务。Mac 的 Swift/AppKit 构建只能在具备 macOS SDK 的 Mac 完成。
