# LAN RemoteApp

在 Mac 上启动并操作运行在 Linux 上的图形应用。应用和文件仍然位于 Linux，不是把 Linux 二进制转换为 macOS 程序。

**状态：开发者预览，非经过真实 Ubuntu → Mac 联调验收的产品。** 本次按要求在云端开发；未连接、部署或测试你的 Ubuntu/Mac。以 `docs/validation.md` 的实际记录为准，不将源码实现与运行验收混为一谈。

## 两条运行路径

- **P0 / Xpra**：Tauri 启动器经严格验证的 SSH 请求 Linux Agent 启动批准应用，使用已有 Xpra 原生客户端显示独立应用窗口。需要两端的 Xpra；不是自研窗口协议验收。
- **P1 / 原生预览**：Linux Agent 在独立 Xvfb + Openbox 会话中运行应用，捕获真实 X11 窗口 PNG，经 SSH 传给 Swift/AppKit `NSWindow`，回传指针、按键、大小与关闭请求。不依赖 XQuartz/Xpra 显示。此链路源代码已提供，Mac 编译与实机效果仍待验证。

首期不改变 Ubuntu 当前 GNOME/Wayland 桌面，不抓取当前桌面已有窗口，不开放公网控制端口。

## 仓库

- `apps/linux-agent`：用户级 Agent、SSH stdio 桥接、持久图形会话。
- `apps/launcher`：Tauri 2 + Vue/TypeScript 设备与应用启动器。
- `apps/mac-windowhost`：Swift/AppKit 原生窗口宿主。
- `crates/`：协议、传输和 Linux 后端等 Rust 组件。
- `deploy/`：人工审核并安装的 Linux 用户服务、白名单示例。
- `docs/design-v1.md`：原始设计原文。
- `docs/wire-contract.md`：当前可实现的 SSH JSON Lines 契约。
- `docs/architecture.md`：实现与设计差异。
- `docs/threat-model.md`：安全边界。
- `docs/acceptance.md`：之后实机联调的验收步骤。

## 使用顺序

1. 阅读 Linux Agent、Mac WindowHost 和 Launcher 目录内 README，按对应平台安装依赖并编译。不要在没有图形运行依赖的 Linux 上将编译成功当作可运行。
2. 在 Linux 本地审核应用白名单配置，只批准固定的绝对可执行文件和固定参数。客户端不能提交任意程序或 shell 命令。
3. 在 Linux 安装用户级 Agent，并启动用户服务。服务仅使用用户私有 Unix socket，不监听公网 TCP。
4. 在 Mac 上先通过可信渠道核对 Ubuntu SSH 主机指纹，并手工建立正常 SSH 登录。程序使用 BatchMode 和严格主机密钥检查，不自动接受未知或变化的主机密钥，不提供密码收集界面。
5. 启动 Launcher，填写已验证主机、SSH 用户和端口，加载批准应用目录，选择 Xpra 或原生模式启动。
6. 断开连接与关闭远端应用不同：关闭窗口会请求远端应用退出；退出/断开客户端不应删除远端工作。原生模式请保留会话 ID，以便再次连接。

## 当前边界

P1 使用 SSH + 版本化 JSON Lines 和无损 PNG，全帧去重与有界队列，而非已完成的 QUIC、Protobuf、H.264、Metal 或 XDamage 增量流。没有宣称中文 IME、Retina 双屏、复杂菜单、GPU 应用、断网恢复矩阵或性能指标已经通过。协议和 UI 应仅公布实际支持的能力。

不要传输密码、私钥、令牌到聊天或仓库。无需管理员权限运行 Agent；软件拥有运行它的 Linux 用户权限，应用白名单不是 OS 沙箱。建议为远程应用使用专用低权限账户。

## 构建与验证

见 `docs/validation.md`。`scripts/check.sh` 用于已装好工具链的开发机器；它不安装依赖、不连接远端、不修改系统服务。Mac 的 Swift/AppKit 构建只能在具备 macOS SDK 的 Mac 完成。
