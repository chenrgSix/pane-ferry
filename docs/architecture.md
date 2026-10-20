# 实现架构与设计差异

## MVP 决策

保留原方案的 Rust 业务核心、Tauri 管理 UI、Linux 用户 Agent、Swift/AppKit WindowHost。Ubuntu 端只创建产品自己的虚拟 X11 会话，不接管 GNOME/Wayland。

为了让首次交付具有可读、可诊断的完整链路，而不是只有接口定义，当前 P1 选择 SSH stdio + JSON Lines（RWP/1）。消息通过可靠 SSH 字节流按序传输，图像使用 PNG base64。协议显式版本，输入和帧有限额。生产优化再迁移 Protobuf/独立媒体流/QUIC；不将 TCP SSH 转发描述为 QUIC 传输。

## 窗口与图像

Agent 发现产品会话内的可见窗口，发送元数据及真实 PNG。服务端显式配置 existing 时，也可按 PID 接入原桌面已有 X11 窗口，使用 XComposite pixmap 捕获。Swift 宿主创建独立 NSWindow，图像通过系统位图绘制。当前没有 Metal 或 VideoToolbox 解码器。PNG 全帧去重适合文本/静态应用，但不等于 XDamage 区域更新、30fps 或 60fps 性能承诺。

窗口销毁后不能接受旧帧；大小变更带 geometry_revision。队列与解码尺寸必须有上限，无法正确解码的帧被拒绝而不是无限缓存。NSWindow 操作在主线程，网络读取不阻塞 UI。

## 生命周期

远端用户服务拥有应用会话；SSH 桥接只拥有连接。用户主动关闭窗口发送协议关闭请求，连接掉线不能当作应用退出。会话重附着用 snapshot 重建状态；同一会话只允许一个控制端。Agent 服务自身被停止、机器重启或 X server 崩溃不保证工作保留，必须如实诊断。

existing 会话只持有 X11 连接和进程身份，不拥有原进程。Agent 停止不结束它；下次连接重新校验 UID、可执行文件与启动时间，避免 PID 重用后误接其他进程。窗口元数据及输入均检查归属，原桌面的其他窗口不进入此会话。

## 原生链路

服务端批准应用目录、SSH 控制、PNG 窗口流和 AppKit 宿主组成唯一路径。启动器先幂等创建会话，再附着原生宿主；会话列表用于跨启动器重启恢复。系统 SSH 临时连接可用于密码登录，应用不收集或保存密码。

## 后续

需要真实设备验证后继续实现：复杂 popup 和窗口树、剪贴板授权与回环抑制、NSTextInputClient Unicode 提交、Retina DPI 重绘、H.264、断线恢复与休眠、开发者签名、公证和升级。没有通过的能力不应在握手中宣称。
