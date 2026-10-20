# LAN RemoteApp

在 Mac 上启动并操作运行在 Linux 上的图形应用。应用和文件仍然位于 Linux，不是把 Linux 二进制转换为 macOS 程序。

**状态：原生链路开发者预览。** 已在 Apple Silicon Mac 与 Ubuntu 26.04 上验证 xterm 的真实画面、键鼠输入、窗口缩放、隐藏恢复和断开重连。复杂应用、中文输入和 Retina 重绘仍待完善，以 `docs/validation.md` 的实际记录为准。

## 运行链路

Linux Agent 在独立 Xvfb + Openbox 会话中运行白名单应用，捕获真实 X11 窗口 PNG，通过严格验证的 SSH 传给 Swift/AppKit `NSWindow`，回传指针、按键、大小与关闭请求。Mac 只使用打包的原生 WindowHost。

首期不改变 Ubuntu 当前 GNOME/Wayland 桌面，不抓取当前桌面已有窗口，不开放公网控制端口。

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

P1 使用 SSH + 版本化 JSON Lines 和无损 PNG，全帧去重与有界队列，而非已完成的 QUIC、Protobuf、H.264、Metal 或 XDamage 增量流。没有宣称中文 IME、Retina 双屏、复杂菜单、GPU 应用、断网恢复矩阵或性能指标已经通过。协议和 UI 应仅公布实际支持的能力。

不要传输密码、私钥、令牌到聊天或仓库。无需管理员权限运行 Agent；软件拥有运行它的 Linux 用户权限，应用白名单不是 OS 沙箱。建议为远程应用使用专用低权限账户。

## 构建与验证

见 `docs/validation.md`。`scripts/check.sh` 用于已装好工具链的开发机器；它不安装依赖、不连接远端、不修改系统服务。Mac 的 Swift/AppKit 构建只能在具备 macOS SDK 的 Mac 完成。
