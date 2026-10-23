# 整桌面模式：当前范围与验证

Mac 工作台现在有两种入口：打开一台设备上的单个应用窗口，或按显示器连接 Ubuntu 当前已登录的 GNOME 桌面。桌面会话拥有独立的会话 ID，使用现有设备配对、QUIC、H.264 视频与桌面输入授权。每个显示器单独列出、连接和恢复，设备 ID 与会话 ID 仍按原有规则隔离。

整桌面会话只通过 Mutter `RecordMonitor` 获取所选显示器的 PipeWire 画面，不创建应用进程或虚拟显示器，不调整 Ubuntu 的分辨率和缩放。Mac 以真实采集像素等比显示；鼠标按捕获像素与该显示器逻辑布局映射。Mac 调整窗口、全屏或关闭窗口，不会向 Ubuntu 发送桌面尺寸变化或应用关闭请求。关闭仅释放该连接的输入、采集和控制权；原账号、应用与桌面继续运行。文字输入复用 GNOME 窗口扩展的焦点文本桥；新版扩展未加载时，画面和鼠标仍可用，但直接文字提交可能暂不可用。

首版仅发布 Ubuntu 当前登录的 GNOME/Wayland 桌面，画面包括该显示器上的 Wayland 和 Xwayland 应用。登录界面、无人登录时的远程唤醒、音频、Mac 整桌面发布尚未实现。超出当前 4096 像素边长或 800 万像素视频预算的显示器暂不列出。桌面画面是 H.264 有损视频，不保证与本机像素完全一致。

2026-10-23 验证：`scripts/check.sh` 完整通过；Rust Agent/网络/协议、Mac Swift 宿主、Tauri/Vue 构建与前端会话隔离测试通过。Ubuntu 26.04 当前真实 GNOME 会话只读实测列出 `DP-2`（1920×1080），Mutter 创建了该显示器的采集流，PipeWire/GStreamer 收到 1920×1080 BGRx 帧，缓冲区为 8,294,400 字节。另在隔离 HOME 和私有 Agent socket 启动新 Linux release 二进制，验证 `desktop.list`、`desktop.start` 和 `session.list` 返回该显示器的独立桌面会话；没有连接输入门户、退出应用或改变原服务。此测试尚未覆盖完整 Mac↔Ubuntu 桌面控制与 H.264 持续帧传输。

当前两台机器的正式运行进程尚未切换：Ubuntu Agent 有受控应用会话，Mac 也有旧宿主窗口。Linux 新二进制仅在 `/tmp/paneferry-desktop-build/target/release/paneferry-agent` 隔离构建，Mac 新包在 `apps/launcher/src-tauri/target/release/bundle/macos/窗渡 PaneFerry.app`。在现有任务结束前，不重启 Ubuntu Agent、GNOME 或这些宿主。正式部署后需重新读取显示器，并在可用时做连接、鼠标、多屏、断开/重连和文字输入的真实验收；构建与隔离测试不能代替该验收。
