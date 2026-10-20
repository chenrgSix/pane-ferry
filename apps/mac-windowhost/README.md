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
- 键鼠、滚轮与 Command→Control 快捷键。NSTextInputClient 在控制端保留预编辑，提交 Unicode 后交给 Agent；普通字母与输入法修饰键不再作为物理按键转给 Ubuntu 输入法。候选框以最近点击位置为锚，尚无精确远端插入点和周边文本协议，复杂输入法/替换范围与非 ANSI 快捷键仍待完整验收。
- 默认自动适应本机窗口，以内容区和本机 backing 像素密度请求远端宽高；拖动中最多每 100ms 合并一次更新，松手发送最终尺寸。远端旧尺寸回包不会拉回本机窗口。自动模式始终铺满内容区，重绘到达前暂时缩放上一帧；超过渲染预算时保留窗口比例降低像素数。
- “显示”菜单可恢复自动适应窗口、适应可用屏幕、全屏或手动输入固定远端像素宽高；标题栏显示实际收到的分辨率。只有手动固定模式保持远端比例并可能留边，鼠标按对应图像区域换算。
- 界面缩放与分辨率分开：新独立会话可按本机像素密度选择 100%/200%，通过 GTK/Qt 环境让应用重新绘制；原桌面窗口沿用其原本缩放。运行中能改变像素宽高，但切换工具包 DPI 需新会话；跨屏幕自动热切换 DPI 尚未实现。macOS 显示缩放下的 backing 像素可能不同于面板标称像素，手动输入可指定精确远端宽高。
- 父子关系有循环检查，但复杂菜单位置、模态阻塞、窗口跨屏和异常弹窗仍待实测。
- 编辑菜单或 Command+C/V 主动复制/粘贴 UTF-8 纯文本，单次最多 8192 字节。粘贴时才读取控制端剪贴板；复制回包必须对应本次请求和当前窗口才会更新控制端剪贴板。不做后台同步，不支持图片、文件或剪切。
- 手动用会话 ID 重连；无自动重连、文件拖放或独立应用 Dock 身份。
- 输入队列、消息长度、PNG 尺寸、窗口数量和待显示解码帧像素有上限；旧 geometry_revision、销毁窗口的旧帧会丢弃。已销毁 XID 在当前连接内复用会被拒绝，需重连重新获取快照。
- 不显示远端 stderr，不把按键/画面/剪贴板写入日志。SSH 失败只显示退出码；在终端手动运行 SSH 核查连接，不关闭严格校验。

## 验证范围

已通过 Swift release 构建、Mac 原生 xterm 显示与输入，以及 Agent 的 20 次 SSH 重连和断开 30 秒后的恢复。待验收：Mousepad 与其他应用、复杂多窗口/模态菜单、跨屏、休眠唤醒、Intel Mac、性能与长期稳定性。完整记录见 `docs/validation.md`。
