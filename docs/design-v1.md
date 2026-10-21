---
title: "LAN RemoteApp｜跨平台无缝远程应用系统"
subtitle: "技术设计方案 v1.0 · Ubuntu 26.04 → macOS 首发 · 2026-10-20"
lang: zh-CN
---

# 1. 项目定位与范围

**产品目标：** 用户在 Mac 上点击一个应用图标，局域网中的 Ubuntu 26.04 在独立图形会话中启动相应 Linux GUI 软件；该应用以可移动、可调整大小、可最小化的 **macOS 原生窗口** 出现在 Mac 桌面上。真实执行、存储和进程生命周期仍在 Linux 上。长期支持 Windows、macOS、Linux 任意组合的应用发布和接收。

**不做：** 不传输完整远端桌面；不要求更换 Ubuntu 的 GNOME/Wayland；不将 GTK/Qt 控件重新绘制成 Cocoa 原生控件；不承诺任何已有桌面窗口都可无损迁移。默认启动独立 X11 应用；现有 X11/XWayland 进程可经服务端显式审批接入，具体边界见 `linux-agent.md`。

## 1.1 接近原生的定义

| 能力 | V1 目标 | 说明 |
|:--|:--|:--|
| 原生窗口 | 必须 | macOS `NSWindow`；标题栏、拖拽、最小化、Mission Control |
| 独立操作 | 必须 | 键盘、鼠标、滚轮、焦点同步、窗口关闭 |
| 清晰度 | 必须 | Retina/HiDPI 映射；避免把低分辨率画面单纯放大 |
| 多窗口/弹窗 | 必须 | 主窗口、对话框的父子关系与层级；复杂菜单允许降级 |
| 基础剪贴板 | 必须 | 纯文本双向复制；图片和文件第二阶段 |
| 中文输入法 | V1.5 | Mac 本地预编辑、远端提交；不能只转发键码 |
| 断线重连 | V1.5 | 应用在 Linux 上继续运行，Mac 可重新附着 |
| 拖放文件 | V2 | 上传/下载、临时映射、授权，不是简单传路径字符串 |
| 独立 Dock 应用项 | V2 | 首期一个主 App 的 Dock 项；以后分离辅助进程/应用包 |
| macOS 全局菜单栏 | 不保证 | Linux 程序菜单不自动转换成原生 NSMenu |

## 1.2 双向产品原则（2026-10-22 更新）

Ubuntu → Mac 的基础链路打通后，下一阶段推进 Mac → Ubuntu，并统一为每台设备都能发布和接收应用的 LAN RemoteApp。

- 每个平台只有一个产品安装包和管理入口，包含统一的本机服务与该平台所需的辅助组件；用户无需选择服务端版或客户端版。
- 发布与接收是可同时启用的两项能力，不设置整机互斥的客户端/服务端模式。Mac 使用 Ubuntu 应用时，也能向 Ubuntu 发布自己的应用。
- 服务核心共用设备配置、连接、应用目录、会话管理与协议；发布后端和接收窗口宿主按操作系统适配。统一产品允许内部使用受管理的辅助进程。
- 应用进程、文件和发布会话属于实际运行应用的机器；关闭接收窗口或连接中断只解除附着。入站与出站会话分别管理，停止发布不应断开本机正在使用的远端应用。
- 支持能力与当前可用状态分开报告：系统权限、登录桌面和组件状态决定能否发布。按会话保持单控制端约束，不把它扩大为整台设备只能承担一种角色。
- Mac 发布首版暂按当前已登录图形桌面设计；独立后台图形会话需另行确认与验证，不能直接照搬 Linux 的 Xvfb 实现。

下文保留首阶段的选型与里程碑作为背景；下一阶段的模块拆分、迁移顺序和验收要求以 [architecture.md](architecture.md#统一节点架构下一阶段) 为准。此节是目标架构，不表示 Mac 发布或 Ubuntu 接收已经可用。

# 2. 架构决策

推荐 **“统一控制核心 + 平台专属窗口宿主 + 可插拔发布后端”**，避免将 XQuartz 或 FreeRDP 固定为产品基础协议。

```text
                       Mac (用户操作)
  ┌────────────────────────────────────────────────────┐
  │ Tauri 2 应用管理界面（设备/图标/设置/会话）         │
  │ Rust Core（发现、认证、协议、会话、能力协商）      │
  │ Swift AppKit WindowHost（NSWindow / Metal / IME） │
  └────────────────────────┬───────────────────────────┘
              控制 + 输入 ↕      图像/音频 ↑
                  QUIC/TLS（首期可先用 SSH）
                           │
  ┌────────────────────────┴───────────────────────────┐
  │ Ubuntu 26.04 : user-level RemoteApp Agent         │
  │ 设备注册 / 应用白名单 / 会话管理 / 策略验证          │
  │ LinuxX11Backend                                  │
  │ 独立 X11 显示会话 + Window Manager + GUI 应用     │
  │ X11 事件监测 / 窗口级捕获 / 编码 / 输入注入         │
  └────────────────────────────────────────────────────┘
```

**单一原生路径：** 自行实现 Linux Window Agent 与 Mac AppKit WindowHost，以版本化控制协议交换窗口、输入和画面。首先验证 SSH + PNG 原型，再扩展增量更新和媒体编码；Mac 使用自有窗口宿主。

## 2.1 为什么不一直使用 X11 Forwarding？

SSH X11 Forwarding + XQuartz 能快速打开远端应用，但通常不能满足持久会话、现代视频编码、HiDPI、多应用窗口 Dock 行为与可靠重连的产品要求。适合验证，不作为最终图形内核。

## 2.2 为什么不从零写 RDP？

RDP/RAIL 是 Windows 应用发布的重要适配器，但给 Linux→Mac 做一个 Windows RDP 兼容服务器既不必要也昂贵。内部定义统一的 `RemoteWindow` 事件模型，Windows 发布端未来通过 RemoteApp/RAIL 或原生捕获适配即可。

# 3. 技术选型（建议）

| 层 | 选择 | 原因与替代 |
|:--|:--|:--|
| 跨平台业务核心 | Rust + Tokio | 网络、状态机、跨平台复用 |
| 主应用界面 | Tauri 2 | 设备列表、应用目录、设置跨 Mac/Windows/Linux 复用 |
| macOS 远程窗口 | Swift + AppKit `NSWindow` | 真正参与 macOS 窗口管理；渲染使用 Metal/`CAMetalLayer` |
| macOS 输入法 | `NSTextInputClient` | 本地输入法预编辑、候选文字和文本提交 |
| Linux 应用运行 | 独立 X11 Session | Ubuntu 本机 Wayland 不改；先支持典型 GTK/Qt/X11 应用 |
| Linux 窗口事件 | Rust `x11rb` + EWMH/X11 扩展 | 检测窗口创建/销毁/属性/焦点/层级 |
| 初始图像捕获 | GStreamer `ximagesrc xid=...` | 窗口级采集，使用 XDamage 减少无效更新 |
| 正式视频编码 | GStreamer H.264 低延迟管线 | 先软编码，按机器能力支持 VA-API/NVENC 等硬件编码 |
| macOS 视频解码 | VideoToolbox | 系统硬件加速解码；输出到 Metal |
| 网络 | MVP：SSH；正式：QUIC/TLS（Quinn） | 单局域网直接连接、独立控制流和媒体流 |
| 消息编码 | Protobuf / prost | 类型、版本和向前兼容明确 |
| 日志 | tracing + 结构化 JSON | 可观测、可排错，不记录私密内容 |
| 服务进程 | Linux `systemd --user` | 非 root；会话与图形进程可脱离连接生命周期 |

**注意：** `ximagesrc` 适合第一版窗口画面采集，但是否能正确呈现菜单、弹窗、透明/形状窗口要通过真实应用验证；正式版可能需要 XComposite + XDamage + XFixes 的专用捕获逻辑。Linux 头less X11 会话需要窗口管理器（例如 Openbox）和完整的环境变量，而不只是 `Xvfb` 一个进程。

# 4. Ubuntu 发送端详细设计

## 4.1 启动与运行隔离

- `remoteapp-agent` 以普通 Linux 用户的 `systemd --user` 服务运行；默认不使用 root、不要求停止本机 GNOME/Wayland。
- 用户在 Mac 点图标后，Agent 从已批准应用目录中解析启动信息，创建/复用应用会话，分配唯一 `session_id` 和虚拟显示号。
- 每个会话启动一个独立的 X11 显示环境（原型：Xvfb；后续根据 GPU 需求替换为适合的 Xorg 虚拟服务器）和轻量级窗口管理器。
- 启动 GUI 程序时传入对应 `DISPLAY` 和隔离的会话环境；窗口在这个远端显示中绘制，不会出现在 Linux 本机当前桌面上。
- V1 每个应用可单独建会话，减少应用间冲突，也便于不同 Mac 显示器/DPI 设置；以后可优化为共享会话。

## 4.2 应用目录

- 从 `.desktop` 文件读取名称、图标、Exec、工作目录、Desktop Entry 分类等信息。
- 仅将管理员或用户明确批准的应用公布给客户端，返回稳定 `app_id`，不直接接受任意 Shell 命令。
- 不执行从客户端传入的任意 `sh -c` 字符串；对进程启动使用参数数组、固定可执行文件及参数校验。
- 应用不存在、依赖不全或不支持当前虚拟显示时返回可诊断的错误。

## 4.3 窗口注册器

`WindowRegistry` 监听 X11 的 Create/Map/Unmap/Configure/Destroy、属性变化、焦点变化，补充 `WM_TRANSIENT_FOR`、`_NET_WM_WINDOW_TYPE`、`WM_CLASS` 和图标等信息。映射为产品统一窗口模型：

- 主窗口 → macOS `NSWindow`。
- 模态对话框 → 父窗口关联的 `NSPanel` 或 Child Window。
- 工具提示/弹出菜单 → 短期 Popup/Overlay，保证层级、坐标和关闭行为。
- 不具备可靠窗口元信息的特殊软件可退回“应用级单一画布”模式，牺牲部分系统感换取功能正确性。

## 4.4 捕获与编码

- V1：按窗口 ID 捕获，以 PNG/无损块更新传输静态 UI；对 `damage` 区域按需刷新。
- V1.5：滚动、动画和视频场景切换为 H.264；编码器采用低延迟参数，客户端只显示最新可解码画面；丢弃积压的过期帧。
- 优先保证代码、文字清晰，避免为了 60fps 把字体压到发糊。
- 窗口最小化或被遮挡时应降低帧率，稳定静止界面不持续发送全帧。
- 对复杂应用（Chromium、Electron、OpenGL）做专门兼容测试；不能保证 Xvfb 上的 GPU/硬件加速与本机完全一致。

## 4.5 输入注入

- 指针：Mac 内容视图坐标 → 远端窗口逻辑坐标 → X11 事件；包含滚轮和多按钮。
- 按键：发送布局无关的物理键标识、修饰键和可输入文本，不能只发本地字符码。
- 普通快捷键经过策略映射（Mac `Command` ↔ Linux `Control`），并允许单应用覆写。
- 中文输入：Mac 用 `NSTextInputClient` 管理 marked text；提交文本经单独协议交给远端输入方法桥接；纯 XTest 键码无法可靠覆盖所有 Unicode 字符，剪贴板粘贴仅作为降级方案。
- 安全注意：不模拟 root/UAC/锁屏等特权界面；控制仅限对应用户的独立会话。

# 5. macOS 接收端详细设计

## 5.1 模块

1. **App Launcher（Tauri）**：主机发现、应用目录、搜索、最近使用、收藏、错误提示。
2. **Session Core（Rust）**：会话连接、自动重试、身份认证、消息序列和能力协商。
3. **WindowHost（Swift/AppKit）**：每个远端 top-level 对应一个 `NSWindow`，处理置前、最小化、缩放、全屏、焦点和辅助窗口。
4. **RemoteSurfaceView（Metal）**：解码后的图像渲染到本地窗口内容区域；窗口 chrome 由 macOS 管理。
5. **InputBridge（AppKit）**：键盘、鼠标、文本输入法、拖放手势和光标同步。
6. **ClipboardBridge（NSPasteboard）**：先支持 UTF-8 纯文本，带回环抑制与敏感内容限制。

## 5.2 原生体验关键技术点

- 远端 `window.created` 创建真正的 NSWindow；用户移动窗口时本地先立即响应，再把几何状态同步到远端。
- 用 `backingScaleFactor` / `NSScreen` 做点与像素映射；移动到 Retina 屏幕时通知远端更新目标分辨率及 DPI。可能需要重新创建渲染资源。
- Mac 红色关闭按钮只断开连接并退出宿主，保留远端应用；真正关闭程序由其画面内的关闭按钮或退出菜单完成。
- 一个 Mac 程序的多个 NSWindow 默认通常属于同一个 Dock 应用；**要做到每个远程软件都是单独 Dock/Cmd+Tab 项，需要拆为独立可识别的 app/helper 进程或应用包**，并解决签名、公证和升级机制，放在 V2。
- 应用内部仍是 GTK/Qt 画面，Mac 菜单栏可以先提供“连接、关闭、复制、粘贴、缩放”等通用菜单，而不是假装可以完整解析所有 Linux 应用菜单。

# 6. 通信协议草案（Remote Window Protocol，RWP v1）

分为 **控制通道、输入通道、媒体通道、剪贴板/文件通道**。稳定控制消息必须按序、可靠投递；媒体包支持新帧覆盖旧帧，避免队列不断增加。每条消息携带 `protocol_version`、`session_id`、`window_id`、递增序号和必要的时间戳。

## 6.1 启动流程

```text
Mac 启动图标
   → Pair/Authenticate（首次认证，存储信任关系）
   → GetAppCatalog / Launch(app_id)
   → Ubuntu 创建 X11 会话并启动应用
   → WindowCreated(session_id, window_id, parent_id, bounds, dpi)
   → Mac 创建 NSWindow 并回传 WindowMapped
   → Ubuntu 推送 PNG/H.264 帧
   ↔ Mac 发送鼠标/键盘/窗口配置事件
   → Mac 断开时：会话保留；再次连接时 Snapshot + Replay
```

## 6.2 典型事件（概念结构，非已有标准）

```json
{
  "type": "window.created",
  "protocolVersion": 1,
  "sessionId": "s_4c71",
  "windowId": "w_0002",
  "parentId": null,
  "role": "toplevel",
  "title": "Mousepad",
  "bounds": {"x": 0, "y": 0, "width": 960, "height": 640},
  "scaleFactor": 2.0,
  "capabilities": ["resize", "focus", "minimize", "clipboard.text"]
}
```

其他必备事件：`hello`、`capabilities`、`app.list`、`app.launch`、`session.snapshot`、`window.metadata`、`window.destroyed`、`window.configure`、`window.focus`、`window.close.request`、`frame.chunk`、`frame.ack`、`input.pointer`、`input.key`、`input.text.commit`、`clipboard.changed`、`session.detach`、`session.resume`。

**防乱序要求：** `window.destroyed` 之后不能再渲染旧帧；调整大小时增加 `geometry_revision`，帧携带对应版本；断线后从 `snapshot` 恢复状态，不用依赖旧连接未确认的事件。

## 6.3 QUIC 的使用边界

- 初始 MVP 可以通过 SSH 安全传输控制和 PNG 帧，方便调试和复用登录认证。
- 正式版采用基于 TLS 1.3 的 QUIC，可靠流承载控制/剪贴板，媒体使用独立流或分片封装；优先在局域网使用直连。
- 原生 QUIC 基于 UDP；**不能假设普通 SSH `-L` TCP 端口转发可以透明承载 UDP/QUIC**。
- 配对时使用可信 SSH 引导、公钥指纹固定或设备证书，后续更新证书需明确信任流程。

# 7. 会话生命周期与可靠性

| 状态 | 含义 | 行为 |
|:--|:--|:--|
| `CREATING` | 生成图形会话 | 等待虚拟 X11/窗口管理器健康检查 |
| `RUNNING` | 应用窗口可用 | 主动推流与处理交互 |
| `DETACHED` | Mac 断开 | Ubuntu 进程保留，采集可暂停 |
| `RESUMING` | Mac 重新连接 | 发送完整窗口快照、关键帧、恢复映射 |
| `EXITED` | 用户退出软件 | 清理图形会话资源，回传退出码 |
| `FAILED` | 运行失败 | 保留诊断日志，给出恢复建议 |

- Mac 最小化不是 Linux 进程退出；退出 Mac 主应用不自动杀死远端程序，除非用户配置如此。
- 由 `systemd --user` 维护 Agent；支持 Ubuntu 端无人登录时运行需要验证用户服务和 linger 的行为。
- `launch` 操作使用幂等请求 ID，避免网络重试造成重复启动。
- 支持每台 Ubuntu 多个客户端，但首期同一应用会话只允许一个活动控制端，防止键盘和焦点冲突。

# 8. 安全与隐私

1. **局域网也必须身份认证和加密**：SSH/QUIC-TLS；发现主机不代表获得控制权。
2. **设备配对**：首次验证设备指纹、授权用户；私钥放 macOS Keychain；支持撤销。
3. **应用白名单**：客户端只能请求经批准的 `app_id`，不能执行任意 shell 命令；参数需结构化验证。
4. **最小权限**：Linux Agent 使用普通用户，不开 root shell；远端 GUI 进程拥有该用户能访问的权限，所以重要数据最好使用独立低权限 Linux 账户隔离。
5. **剪贴板与文件权限**：默认纯文本剪贴板，文件传输单独授权；企业模式可关闭。
6. **服务发现**：mDNS 只广播服务名/版本，不包含身份密钥、文件或敏感应用信息。
7. **日志**：保留连接、错误、窗口生命周期、权限变更；不记录按键内容、剪贴板数据或远端画面。
8. **传输策略**：首次只在可信局域网开放明确端口；不把 SSH 或自研控制端口直接暴露公网。

# 9. 性能指标与测试方法

以下是**验收目标，而非已经测得的性能**。在同一千兆局域网、正常硬件和有限丢包条件下测量。

| 指标 | 目标 |
|:--|:--|
| 基础应用启动 | 客户端发起后显示第一窗口 ≤ 5 秒（冷启动条件单独统计） |
| 静态界面画质 | Retina 上文字清晰，无持续模糊和明显色块 |
| 输入到画面变化 | 中位数 ≤ 60 ms；P95 ≤ 100 ms（测试目标） |
| 拖动/滚动 | 日常窗口交互 30fps，流畅场景争取 60fps |
| 断线恢复 | 应用不退出；恢复后窗口结构和焦点可重建 |
| 空闲带宽 | 静止窗口时接近零视频数据（心跳除外） |
| 运行平台 | Ubuntu 26.04 发送端；Apple Silicon Mac 优先；Intel Mac 单独验证 |

**测试用例矩阵：** xterm（基础 X11）；Mousepad（GTK、输入法）；Firefox/Chromium（多窗口、菜单、图形加速）；LibreOffice（弹窗、文件）；双显示器；Retina 1x/2x；中英文输入；20 次连续重连；应用崩溃；窗口快速缩放；网络瞬断；Mac 休眠唤醒。

# 10. 代码仓库建议结构

```text
lan-remoteapp/
├── apps/
│   ├── launcher/                # Tauri 2 管理与应用列表
│   ├── mac-windowhost/          # Swift AppKit, Metal, IME
│   └── linux-agent/             # Linux systemd user service
├── crates/
│   ├── protocol/                # protobuf 与能力版本
│   ├── core/                    # 设备、应用、会话状态机
│   ├── transport/               # SSH/QUIC、安全认证
│   ├── backend-x11/             # X11 会话、窗口事件和输入
│   ├── capture/                 # XDamage、采集和图像编码
│   ├── clipboard/               # 剪贴板 MIME 与回环抑制
│   └── observability/           # tracing、指标和日志
├── proto/
│   ├── control.proto
│   ├── window.proto
│   └── input.proto
├── deploy/
│   └── linux-user-service/
├── tests/
│   ├── protocol/
│   ├── compatibility/
│   └── e2e/
└── docs/
    ├── architecture.md
    ├── protocol.md
    └── threat-model.md
```

# 11. 里程碑与人力成本

以下周期假设有 **2 名熟悉 Rust/Linux 和 macOS 原生开发的工程师 + 兼职测试**；仅用于项目计划，不是确定交付承诺。

| 阶段 | 交付内容 | 估算 |
|:--|:--|:--|
| P0 可用性验证 | Mac 图标 → SSH/PNG → AppKit 独立窗口；兼容矩阵 | 1–2 周 |
| P1 原生单窗口 | Ubuntu 虚拟 X11 Agent + PNG 图像 + Mac NSWindow + 输入 | 3–5 周 |
| P2 多窗口与稳定性 | 窗口树、弹窗、剪贴板、重连、错误恢复 | 4–6 周 |
| P3 体验优化 | H.264/VideoToolbox、HiDPI、IME、快捷键 | 4–8 周 |
| P4 产品化 | 自动安装、升级、诊断、设备认证、性能/安全测试 | 4–8 周 |

**可展示的完整 MVP：** 约 2–3 个月；**具有良好日常使用体验的版本：** 约 4–7 个月；复杂 App/GPU/输入法兼容性可能明显拉长进度。

## 11.1 每阶段验收门槛

- P0：在 Mac 双击图标能打开 Ubuntu xterm/Mousepad，整个 Ubuntu 桌面保持原样。
- P1：Mac 上真正创建原生 `NSWindow`，**不依赖 XQuartz 进行远程窗口显示**。
- P2：主窗口+对话框能正确配合；断线 30 秒后应用仍在，重新连接状态一致。
- P3：中文输入、Retina 2x、快速滚动、剪贴板在真实应用场景稳定。
- P4：安装、更新、权限撤销、崩溃诊断和中断恢复形成可交付软件。

# 12. 未来三平台扩展

| 软件运行端 | 推荐发布后端 | 接收端窗口宿主 |
|:--|:--|:--|
| Linux（首发） | X11 backend；未来增加 Wayland/PipeWire portal | Mac AppKit / Windows Win32 / Linux GTK/Qt |
| Windows | 优先正式 RemoteApp/RDS 适配；也可自研 Windows Graphics Capture 单窗口后端（须处理安全桌面与权限限制） | 同一个统一窗口协议模型 |
| macOS | ScreenCaptureKit 捕获应用窗口 + 合规的输入事件机制（需用户授权） | 同一个统一窗口协议模型 |

控制层可以复用 `AppCatalog`、`LaunchSession`、`RemoteWindow`、`InputEvent`、`ClipboardItem`、`Transport`；窗口捕获和输入注入是平台专有实现。未来若不要求兼容 RDP，就维持统一的自有数据面协议；若需要兼容企业 Windows RDS，则将 FreeRDP/RAIL 作为单独适配器，不污染核心协议。

## 12.1 Windows 与 macOS 的重要限制

- Windows Windows.Graphics.Capture 和 macOS ScreenCaptureKit 都需要满足系统级捕获及授权要求，不能假设所有受保护的窗口都可捕获。
- macOS 的 ScreenCaptureKit 适合捕获窗口，但向远端 macOS 输入控制属于另一套权限和 API；不能把“能录屏”等同于“可任意远控”。
- Wayland 发送端通常必须经过桌面 Portal、PipeWire 或指定合成器后端；不要假设它提供 X11 式任意窗口枚举和全局输入注入。
- 同一软件在不同系统上做图标、Dock、文件关联、菜单栏等深度集成需要额外的平台工作。

# 13. 最重要的五个技术风险

1. **单窗口截图不等于完整应用**：文件对话框、悬浮菜单、右键菜单、模态窗口必须有窗口树和层级同步。
2. **HiDPI 不是单纯缩放位图**：需要远端重新布局/重绘与编码宽高协商，尤其是跨 1x/2x 显示器。
3. **中文输入法不是键盘事件转发**：必须实现组合输入到远端文本提交的桥接。
4. **GPU 应用可能不适合 Xvfb**：要提供高级 GPU/Xorg/Wayland 发布后端或兼容性降级。
5. **安全与系统整合存在权衡**：任意文件访问、任意远端命令和剪贴板同步都可能扩大攻击面；“原生感”不应以无限权限换取。

# 14. 最终决策（ADR）

**首阶段基线：** Rust 共享核心、Tauri 启动器、Ubuntu Linux Agent、Swift/AppKit WindowHost；以自主的窗口协议验证 Linux → Mac 的原生应用体验，不改变 Ubuntu 当前桌面。

**下一阶段（2026-10-22）：** 统一每个平台的产品与本机服务，抽离发布/接收的平台实现，补齐 Mac → Ubuntu，并验收同一设备同时承担两种角色。现有 Ubuntu → Mac 能力需要持续回归。

**暂缓：** 完整 Mac 菜单栏集成、文件拖放、4K 多路 60fps、原生 Wayland 窗口抓取、跨互联网打洞和 Windows 双向适配。独立 Dock 名称与图标的当前实现见 Mac WindowHost README。

**成功标准：** 用户从 Mac Dock 点击图标，Ubuntu 启动应用，Mac 收到原生独立窗口；输入自然、字清晰、窗口和弹窗行为正确、断线不会导致丢工作内容。若体验不稳定，先优先修复基础窗口语义，而不是继续叠加更多协议或 AI 功能。

# 15. 参考资料（官方或源代码文档）

- FreeRDP/RAIL：https://github.com/FreeRDP/FreeRDP
- x11rb：https://github.com/psychon/x11rb
- GStreamer ximagesrc：https://gstreamer.freedesktop.org/documentation/ximagesrc/index.html
- GStreamer x264enc：https://gstreamer.freedesktop.org/documentation/x264/index.html
- Apple NSWindow：https://developer.apple.com/documentation/appkit/nswindow
- Apple NSTextInputClient：https://developer.apple.com/documentation/appkit/nstextinputclient
- Apple VideoToolbox：https://developer.apple.com/documentation/videotoolbox
- Quinn QUIC：https://docs.rs/quinn/latest/quinn/

