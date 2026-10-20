# 实现架构与设计差异

## MVP 决策

保留原方案的 Rust 业务核心、Tauri 管理 UI、Linux 用户 Agent、Swift/AppKit WindowHost。Ubuntu 端只创建产品自己的虚拟 X11 会话，不接管 GNOME/Wayland。

为了让首次交付具有可读、可诊断的完整链路，而不是只有接口定义，当前 P1 选择 SSH stdio + JSON Lines（RWP/1）。消息通过可靠 SSH 字节流按序传输，图像使用 PNG base64。协议显式版本，输入和帧有限额。生产优化再迁移 Protobuf/独立媒体流/QUIC；不将 TCP SSH 转发描述为 QUIC 传输。

## 窗口与图像

Agent 发现产品会话内的可见窗口，发送元数据及真实 PNG。Swift 宿主创建独立 NSWindow，图像通过系统位图绘制。当前没有 Metal 或 VideoToolbox 解码器。PNG 全帧去重适合文本/静态应用，但不等于 XDamage 区域更新、30fps 或 60fps 性能承诺。

窗口销毁后不能接受旧帧；大小变更带 geometry_revision。队列与解码尺寸必须有上限，无法正确解码的帧被拒绝而不是无限缓存。NSWindow 操作在主线程，网络读取不阻塞 UI。

## 生命周期

远端用户服务拥有应用会话；SSH 桥接只拥有连接。用户主动关闭窗口发送协议关闭请求，连接掉线不能当作应用退出。会话重附着用 snapshot 重建状态；同一会话只允许一个控制端。Agent 服务自身被停止、机器重启或 X server 崩溃不保证工作保留，必须如实诊断。

## P0 与 P1 分离

Xpra 是 P0 实用基线，不是 P1 的私有协议。两条路径共用服务端批准应用目录和严格 SSH 连接配置。P0 依赖 Xpra 客户端，P1 使用项目 WindowHost；禁止把 HTML5 Xpra、整桌面 shadow 或本地测试 PNG 标作原生链路通过。

## 后续

需要真实设备验证后继续实现：复杂 popup 和窗口树、剪贴板授权与回环抑制、NSTextInputClient Unicode 提交、Retina DPI 重绘、H.264、断线恢复与休眠、打包签名和升级。没有通过的能力不应在握手中宣称。
