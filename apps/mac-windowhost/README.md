# macOS WindowHost（P1 开发预览）

真实 Swift/AppKit `NSWindow` 宿主，接收 Linux Agent 的 PNG 窗口画面，通过严格校验主机密钥的系统 SSH 发送输入。已在 Apple Silicon Mac 与 Ubuntu 26.04 上验证 xterm 画面、实际键盘操作、启动器重启后恢复既有会话。仍是开发预览，复杂应用与显示场景未完成验收。

## 构建与运行

macOS 12+，Xcode Command Line Tools / Swift 5.9+。先在 Ubuntu 安装并配置 Agent，在 Mac 手动核实 SSH 主机指纹，确认能用现有密钥/ssh-agent 登录。WindowHost 不生成密钥，不自动保存密码，不会接受未知主机。

```sh
cd apps/mac-windowhost
swift build -c release
.build/release/RemoteAppWindowHost --host 192.168.1.20 --user alice --port 22 --app mousepad
# 重新附着既有会话：
.build/release/RemoteAppWindowHost --host 192.168.1.20 --user alice --session SESSION_ID
# 生成供启动器使用的未签名开发 .app：
./scripts/build-app.sh
```

默认读取 OpenSSH 的 known_hosts 与已有密钥/ssh-agent。`-F /dev/null` 禁用用户 SSH 配置，因此这里必须用真实主机名/IP，不能仅填自定义 SSH 别名；不支持 ProxyCommand 跳板。参数无 shell 插值；远端入口固定为 `.local/bin/remoteapp-agent bridge`。

也会复用 `~/.ssh/lan-remoteapp-%C` 的现有项目 SSH 连接。密码用户可先在项目根目录运行 `bash scripts/connect-ssh.sh HOST USER 22`，由系统 SSH 完成登录，不向 WindowHost 提供密码。

菜单“复制会话 ID”只在用户点击时写入 Mac 剪贴板，用于手动重连。菜单“断开并退出”保留 Linux 会话；窗口红色关闭按钮则请求 Linux 程序关闭，可能触发未保存文件确认。关闭最后一个远端窗口不会退出连接宿主。

## 当前能力与限制

- 每个远端窗口一个 NSWindow；窗口标题、大小、焦点、请求关闭、基础父子窗口层级。
- PNG 使用 NSView/CoreGraphics 绘制，**不是 Metal/VideoToolbox**。
- 键鼠、滚轮、基础 ANSI 物理键、Command→Control。中文预编辑与 Unicode 文本提交尚未实现；非 ANSI 布局未验收。
- 画布按远端像素缩放；未协商远端 DPI，因此**不是 Retina 原生分辨率重绘实现**。
- 父子关系有循环检查，但复杂菜单位置、模态阻塞、窗口跨屏和异常弹窗仍待实测。
- 手动用会话 ID 重连；无自动重连、文件拖放、远端剪贴板同步或独立应用 Dock 身份。
- 输入队列、消息长度、PNG 尺寸、窗口数量和待显示解码帧像素有上限；旧 geometry_revision、销毁窗口的旧帧会丢弃。已销毁 XID 在当前连接内复用会被拒绝，需重连重新获取快照。
- 不显示远端 stderr，不把按键/画面/剪贴板写入日志。SSH 失败只显示退出码；在终端手动运行 SSH 核查连接，不关闭严格校验。

## 验证范围

已通过 Swift release 构建、Mac 原生 xterm 显示与输入，以及 Agent 的 20 次 SSH 重连和断开 30 秒后的恢复。待验收：Mousepad 与其他应用、复杂多窗口/模态菜单、跨屏、休眠唤醒、Intel Mac、性能与长期稳定性。完整记录见 `docs/validation.md`。
