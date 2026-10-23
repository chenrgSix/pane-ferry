# 交付验证记录

日期：2026-10-20 至 2026-10-22。执行环境：Apple Silicon Mac（macOS 27.0.1）与局域网 Ubuntu 26.04.1 x86_64。当前只保留自有原生窗口链路。

## 本地构建与检查

| 检查 | 结果 | 范围 |
| --- | --- | --- |
| `cargo test --workspace --locked` | Mac / Ubuntu 各 32 项通过、0 失败，4 项 Linux 专用测试默认忽略 | 协议 13、transport 6、Agent 8、capture 3、core 2；4 项另在 Ubuntu 显式执行通过，不是 36 个端到端场景 |
| `cargo fmt --all -- --check` | 通过 | 根 workspace Rust 格式 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | 根 workspace 静态检查 |
| `npm test` | 4 项通过 | 待确认启动请求的幂等 ID、持久保存与容量限制；明确失败与未知结果的重试区分 |
| `npm run build` | 通过 | Vue TypeScript 类型检查及 Vite 生产构建 |
| `tests/e2e/ssh_helper_validation.py` | 11 个非法参数案例通过 | SSH 登录辅助脚本参数校验 |
| `swift build` 与短消息 pipe smoke | 通过 | AppKit 编译；短 SSH 消息无需等待完整缓冲区、EOF 与分段读取 |
| `tests/e2e/controller_input_smoke.py` | 通过 | 真实 NSTextInputClient 预编辑不出控制端、Unicode 提交、按需复制/粘贴、大小边界与回包关联；像素/点数换算、自动铺满与固定等比模式的点击；AppKit event-tracking 模式中的尺寸同步、迟到几何回包隔离及手动/自动切换；测试不触碰系统剪贴板 |
| `tests/e2e/hidpi_smoke.py`（Ubuntu） | 通过 | GTK 1×/2× 实际缩放、逻辑尺寸保持、PNG 像素翻倍、EWMH resize 与输入；WM/未显示辅助窗口不发布 |
| `tests/e2e/video_stream_smoke.py`（Ubuntu） | 通过 | 真正 H.264 协商、IDR/P 帧、视频期间输入、641×483 奇数尺寸与编码流重建、重连关键帧、静止窗口重连首次快照 |
| `tests/e2e/video_decoder_smoke.py`（Mac） | 通过 | 解码 Linux 导出的 17 个真实视频包，全部使用系统硬件解码；连续 P 帧、输入后像素变化、奇数尺寸裁边、动静态重连及非法 NAL 拒绝 |
| Linux 专用采集/编码测试 | 4 项显式执行通过 | 新旧编码器 9 帧解码 RGB 完全相同，输入像素不变及释放租约；MIT-SHM 与 socket 快照一致、缩放和失败回退；DAMAGE 重置、授权窗口过滤、静态快照及缺扩展回退 |
| `tests/e2e/text_latency_smoke.py`（Ubuntu） | 通过 | 真实 GTK 编辑器对比物理按键/文字提交、30 次连续提交、退格顺序屏障、中文与 emoji；隔离 HOME 与 Xvfb |
| `tests/performance/end_to_end.py`（Mac ↔ Ubuntu） | 5 组、450 个有效样本通过对应画面与阶段校验 | 产品输入处理函数 → 实际 SSH/X11/H.264/硬件解码 → AppKit 绘制完成；3 种输入、分辨率/持续更新/静止与物理路径对照；不含物理面板呈现 |
| `apps/launcher/scripts/build-macos.sh` | 通过 | Tauri release 应用及内嵌 WindowHost.app 打包；本机开发包未签名/公证 |
| shell/Python 语法检查 | 通过 | 构建、检查和 smoke 脚本 |

协议测试包含版本与字段校验、消息长度和尺寸上限、输入契约、仅接受原生后端、主机与应用参数校验。根 workspace 测试不包含 Tauri 原生 crate；上表单独记录其真实 release 构建。

## Ubuntu 与 Mac 实机联调

Ubuntu 上已编译并安装用户级 Agent。xterm 白名单由 Agent 读取，运行依赖包括 Xvfb、Openbox、xauth 和 dbus-run-session。状态目录与 socket 权限检查通过。仅原生后端版本重新部署后，20 次重连 smoke 再次通过。

自动化原生会话 smoke 已通过以下场景：

- 从真实 X11 窗口获取 PNG，并核验图像尺寸；幂等启动和独占控制端校验。
- 通过协议输入命令，在远端创建测试文件并核验；指针按钮和 Shift 到达 X11，中断连接后释放输入。
- 调整窗口大小与几何版本；隐藏、重显保留同一窗口 ID。
- 20 次重复重连，以及一次断开 30 秒后的恢复；重附着收到窗口快照和新图像。
- 经窗口管理器关闭测试 xterm，收到 session.exited，运行中会话列表不再包含它。

Mac 原生界面已实际显示远端 xterm。新版启动器点击应用卡片创建窗口后，输入 `echo native-only-ok` 看到远端输出；退出宿主、刷新会话并重新连接后，同一终端内容保留。启动器本身退出重开后的会话发现也已验证。SSH 用户名输入框失焦后保持小写；密码通过系统 SSH 处理，不写入应用配置。

现有窗口接入另经 `tests/e2e/existing_window_smoke.py` 验证：只捕获目标进程，拒绝其他窗口的输入与错误可执行文件，重复请求复用会话，原有进程在 Agent 停止、重启后仍然存活。测试使用临时 HOME 与自建 X11 桌面，不加载用户桌面配置。

现有窗口接入版本曾接入 Ubuntu 原桌面正在运行的 ChatGPT，验证数字输入、清空草稿与重连，未发送聊天消息。新版在该旧窗口上遇到无输入响应，原因尚未确定；期间另确认桌面锁屏及系统远程输入授权会阻止输入。按用户要求断开并停止测试服务，随后改用 Firefox，未继续操作 ChatGPT。不能将其他应用的通过当作 ChatGPT 已完整验收。

`tests/e2e/existing_window_smoke.py --controller-text` 已在临时 HOME、自建 Xvfb/Openbox 与 GTK TextView 上验证按顺序提交中文/emoji、控制端粘贴、多行 UTF-8、主动复制回传、无选中文字时拒绝返回旧剪贴板，以及超时后继续输入。已修正移动鼠标反复激活窗口、每个按键重复设置焦点的问题；新增“移动鼠标不得抢占另一窗口焦点”的回归检查通过，完整测试再次通过。`native_session_smoke.py` 的真实键鼠、缩放、隐藏及 3 次重连也通过。

原桌面 Firefox 154 使用独立测试配置及 `tests/fixtures/controller-input.html`，未使用默认 Firefox 资料。用户在 Mac 转发窗口上确认点击、中文输入、双向复制粘贴均可用；另观察到多行中文/emoji 粘贴完整回显，页面点击计数、复制/粘贴事件计数与时钟持续更新。剪贴板仍只由主动操作触发。Firefox Snap 的测试实例使用启动器支持的 `DISABLE_WAYLAND=1`，以及私有 Xauthority；未关闭认证或更改默认浏览器配置。

独立 Firefox 高清会话已在 Mac 窗口实测自动适应可用屏幕及手动 2560×1664：标题栏确认实际远端像素宽高，画面保持比例，点击计数增加，中文/emoji 主动粘贴完整回显。分辨率不再以原窗口宽高机械翻倍表示；100%/200% 只控制界面缩放。macOS backing 像素可能不同于面板标称像素，手动设置可指定精确宽高。

自动适应现为默认模式，重新连接后不继承上一宿主的固定分辨率。Mac 实机确认 Firefox 在普通窗口 2940×1612 与全屏 2940×1846 间切换时同步重绘，退出全屏恢复原尺寸，画面铺满无黑边；点击计数和中文/emoji 粘贴再次通过。连续缩放调度另由真实 NSWindow 的 event-tracking 回归测试验证；旧尺寸回包不再反向调整本机窗口。超出 4096×2160 或 8M 像素预算时按本机窗口比例降低渲染像素数。

本轮定位并修正了 WM 内部窗口和从未显示的辅助窗口触发隐藏窗口聚焦、重复 `request_failed` 弹窗的问题；迟到的隐藏/销毁窗口请求使用 `window_unavailable`，不再阻塞界面。另修复 Firefox 恢复到超限最大化尺寸时不发布主窗口的问题，通过 EWMH 解除最大化并调整至预算内。新版重新完成原生会话键鼠、resize、隐藏恢复及 3 次重连测试。其他实际失败仍保留错误提示，不能将此记录解释为所有应用错误均已解决。

2026-10-21 追加回归：`stale_launch_smoke.py` 在隔离 HOME 上验证失效 PID 目录标记、明确错误码、已结束/回收请求 ID 及使用新 ID 启动。`input_latency_smoke.py` 在 Ubuntu 隔离的 2560×1664 GTK 动态窗口上，连续 32 次指针移动后发送按键，5 次样本的旧版/新版输入中位数为 2.3/2.0ms，收到对应 PNG 的中位数为 85.7/48.8ms；每帧约 1.50MB。新版暂停读端制造背压时，远端仍在 11.1ms 响应按键。测量使用缓冲 socket 读取；早先逐字节读端产生的秒级数字已作废。该测量不含 LAN、Mac 解码与显示耗时，不代表视频性能、真实操作到屏幕延迟或 CI/生产验收。高清 resize/输入回归再次通过。

H.264 追加验证：Ubuntu 的动态 GTK 测试中，前 10 帧（含首个关键帧）合计约 304KB，首个关键帧约 248KB，普通视频帧约 1.3KB；同类 PNG 完整帧约 1.5MB。独立输入/画面响应测试中，5 次样本的输入中位数 1.9ms，收到对应视频画面中位数 79.8ms，每个响应包平均 547 字节，读端暂停时输入响应 0.3ms。后者只测 Ubuntu 本机路径，不包含 LAN 和 Mac 显示耗时；不能以带宽下降推断所有环境延迟必定低于 PNG。Mac 实机的 Firefox 已协商并显示 H.264 窗口视频；组件与实际模式可在窗口副标题识别。界面注入工具在点击/粘贴时超时，未将该次尝试记作中文粘贴实机通过；控制端输入与自动适应的独立回归均再次通过。

文字输入追加验证：相同 GTK 编辑器中，修复前物理按键中位数 0.5ms、文字提交中位数 69.1ms，30 次连续文字提交 2131ms。仅 Wayland 桥接保留修饰键间隔；原生 X11 去掉每次粘贴固定的 48/72ms 等待，以 X11 fd 就绪唤醒 selection 处理，并合并排队中同一窗口的相邻文字提交。修复后按键中位数 0.8ms、文字提交 1.6ms，30 次提交 10.3ms；UTF-8 字节上限和按键/粘贴/窗口切换屏障保持有效。数字是 Ubuntu 本机输入交付，不含网络、编码、解码和屏幕呈现。现有窗口/Unicode/剪贴板回归重新通过；期间定位到旧夹具将随机临时路径里的下划线当作无 Shift 物理键输入，已在旧二进制上强制下划线路径复现，再修正夹具并通过，不能当作本轮产品回归。

2026-10-21 全链路追加测量：2940×1612 持续更新窗口中位数 149.05ms、P95 220.57ms；2560×1664 为 123.58/176.50ms，1280×832 为 83.69/126.85ms。当前高清组的远端采集与等待/执行编码合计平均约 107ms，占总平均耗时约 68%。静止组发生一次 644.72ms 样本，其中 553.45ms 在 SSH/网络/读端缓冲剩余段，对应 980 字节普通视频包。另确认当前 10.0.1.66 路由经过 utun4；物理 LAN 对照减少传输段尾延迟，但总耗时因其他阶段波动未改善。详细方法、5 组结果和限制见 [全链路延迟记录](performance/2026-10-21-latency.md)，并保存 450 个逐样本数据。测试使用独立 Agent/自有 GTK 窗口，不部署产品修改或重启现有服务；不能将结果视为现有 Firefox/ChatGPT 应用特定性能或高速摄像机 input-to-photon 测量。

2026-10-21 远端采集/编码优化：使用 MIT-SHM 私有 fd、只读像素租约直接交给系统 GStreamer、XDamage 重绘唤醒，保持原像素宽高及全部画质配置。新旧两版本八组共 720 个有效样本；2940×1612 持续更新中位数 163.04→115.37ms，2560×1664 持续更新两轮合并 141.46→100.21ms，静止后输入 125.56→79.72ms。2560 动态首轮中位数无明显改善、两轮合并 P95 226.44→244.75ms，全部数据均保留，不能宣称尾延迟已解决。完整阶段、画质和回退检查见 [优化后延迟记录](performance/2026-10-21-optimized-latency.md)。正式非计时 Agent 已部署，安装后真实 SSH 键鼠/缩放/隐藏及三次重连通过；独立 Firefox 恢复到 2940×1612、界面 200%、自动适应及 H.264，中文/emoji 粘贴和点击计数实机通过。ChatGPT 原窗口未操作。

## 2026-10-21 Ubuntu 应用发现与窗口列表

本轮位于 `feat/ubuntu-app-discovery`，从原有 SSH / H.264 稳定链路实现，未合并 WebRTC 实验。Ubuntu 用户级 Agent 与 Mac 启动器均已更新。应用发现与接管范围见 [应用与窗口发现](application-discovery.md)。

本地 `cargo test --workspace --locked` 为 **34 项通过、0 失败、4 项 Linux 专用测试忽略**（Agent 8、backend 1、capture 3、core 2、protocol 13、transport 7）。根 workspace 格式与 Clippy 检查通过；前端 4 项测试、Vue 类型检查和 Vite 生产构建通过；Swift release 及 Tauri 完整 macOS 应用包构建通过。此处替代上方历史记录中的 Mac 32 项计数，不表示 CI 或所有应用兼容性验收。

Ubuntu 上的 `tests/e2e/app_discovery_smoke.py` 使用独立 HOME、XDG 应用目录和 Xvfb，已通过：

- 没有 `apps.json` 仍读取真实 `.desktop` 入口；中文名称、SVG 转 PNG、用户目录覆盖、Hidden / NoDisplay / TryExec / OnlyShowIn 过滤，以及增删入口后的即时刷新。
- 真实发现的 XTerm 入口启动，以及控制端提交中文与 emoji 后，远端终端程序实际读取到完全一致的 UTF-8 内容。新终端的剪贴板快捷键仅在该实例生效。
- 只读窗口列表、稳定窗口 ID、名称和图标；同一 PID 的三个窗口中，仅选中主窗口及其对话框进入控制范围，另一主窗口拒绝输入。
- 选中窗口中文输入、显式双向文本剪贴板；关闭目标后会话结束，同进程另一个窗口及原进程继续存活，Agent 停止也不结束原有应用。

原有 `existing_window_smoke.py --controller-text`、`video_stream_smoke.py` 和 `stale_launch_smoke.py` 在 Ubuntu 隔离环境重新通过；视频测试收到 17 个 H.264 包，并覆盖输入、奇数尺寸和重连。

真实 Ubuntu 桌面只读取元数据：发现 **48 个已安装应用、48 个 PNG 图标、3 个桌面窗口**，另保留 4 个自定义入口。实际 Mac 启动器已显示应用图标、中文名称、搜索及独立的“运行中的桌面窗口”区。未接管、聚焦或输入原桌面的 ChatGPT、uTools 和飞书。

从真实已安装列表的 XTerm 卡片新建独立会话后，Mac 原生窗口显示 **2938×1603、界面 200%、H.264**；控制端输入和主动粘贴命令后，画面实际回显 `discovery-ui-final-ok`。终端正常退出后宿主曾显示 `session_exited` 提示，该退出提示仍待单独改善。测试会话和测试进程均已清理，最终会话数为 0。Agent 服务为 active，状态目录 0700、socket 0600；安装二进制 SHA-256 为 `58fab7a2e8dfac4afa95096bcf9555b41eb3215b6282885ec0b7043551905aae`，更新前版本已备份。

窗口发现和接管当前支持 X11 / XWayland；原生 Wayland 窗口暂不支持。48 个入口成功发现不等于全部应用的独立启动及完整交互均已验收。

## 2026-10-21 Firefox 连续输入与关闭行为

用户报告 Firefox 百度搜索框在跳转、关闭页面等操作后，控制端候选框正常，但中文提交不上屏。排查使用独立 Firefox 资料、自有 Xvfb 和仅监听 loopback 的测试页，未操作原桌面 ChatGPT 或其他已登录内容。校准后的旧版测试在地址栏提交新 URL 后仍导航至旧地址；单独保留原焦点没有解决。仅增加剪贴板广告间隔仍在第 5 轮失败，配合 Firefox 专用快捷键节奏后通过完整循环。由此定位到本实现对 Firefox 异步剪贴板与快速快捷键的交付时序处理不足；这不等于已经重现百度页面上当时的全部具体操作。

当前修复保留已聚焦的子窗口，对 Firefox 新 selection 留出 16ms，再以每个转换 4ms 的节奏发送复制/粘贴快捷键；提前读取的 selection 数据也计入完成判断，避免等待不存在的第二次读取。普通 X11 GTK 应用保留原快速路径。新建自有应用显式使用 GTK simple / Qt compose 和 `@im=none`，不会继承桌面 IM daemon；现有应用环境不被改写。Mozilla 也记录了 GTK/X11 快速键序列的合并问题，见 [Bug 1098981](https://bugzilla.mozilla.org/show_bug.cgi?id=1098981)；本次交付结论以本项目的实际对照和回归为准。

`tests/e2e/browser_text_smoke.py` 的最终版本在 Ubuntu 通过：

- 物理字母输入校准；8 轮中文/emoji 提交、主动复制/粘贴和地址栏导航。
- 新建标签输入中文、关闭标签后回到原页面继续输入中文；40 次连续 Unicode 提交完整保留。
- `session.detach` 后同一会话恢复，内容保留，继续提交中文成功；实际收到 H.264 帧，重连后仍收到 H.264 帧。
- 即使服务进程继承 Fcitx 环境，新建测试 Firefox 进程仍使用本项目指定的输入上下文。

最终改动另通过根 workspace Rust 34 项测试（4 项 Linux 专用忽略）、Clippy、格式与 Python 语法检查；Ubuntu 的现有窗口/显式双向剪贴板及应用发现/新建 XTerm 中文回归通过。普通 GTK 本机输入测量 5 个样本的文字中位数 0.6ms、按键中位数 0.2ms，30 次连续文字提交 1.6ms，仅作为快速路径回归，不含 LAN、Mac 解码或显示，也不是 Firefox 的全链路延迟。Firefox 专用兼容等待会增加提交端开销，本次未重测完整 input-to-photon 延迟。

Mac 红色关闭按钮现调用宿主的断开退出流程，不发送 `window.close.request`。真实 `NSWindow.performClose` 回归确认只触发一次断开回调、没有远端关闭请求，远端销毁窗口时也不会反向触发断开。另用真实 Ubuntu 会话验证断开后控制权释放、同一 session/window ID 可重连，最后只清理本轮自建测试终端。完整 Mac GUI 点击自动化曾因界面工具长时间超时而中断，未将那次尝试记作完整 UI 验收。

Swift release 与 Tauri macOS 包构建通过，新的 WindowHost 已安装。Ubuntu 正式非诊断 Agent 已备份更新，服务 active；升级前确认没有控制端和自有应用会话，2 个外部原进程升级后身份不变。新服务读取 48 个应用、48 个图标和 1 个可见原桌面窗口，测试会话已清理。安装 Agent SHA-256：`d8f78615d00e474f7bf2f83988fa5e2f0cb6f8393b988bece9f2bb0f1a2ec798`。服务重启会刷新内存中的会话目录，需要在启动器重新读取应用与窗口；原桌面应用继续保留。需要结束远端程序时，应使用其画面内的关闭按钮或退出菜单。

## 2026-10-21 剪切后连续输入 a

用户报告剪切后连续出现 `a`。控制端旧代码转发 Command+A 的按下事件，依赖后续 A 抬键事件释放；若该事件缺失，松开 Command 只释放远端 Ctrl，A 仍处于按下状态，X11 自动重复便会输入字母。新增真实 `SurfaceView` / `NSEvent` 回归省略 A 抬键事件，旧版在“Command+A 应完整结束”断言失败。修复将每次 Command 快捷键在同一次回调内完成按下/释放，先释放字母再释放修饰键；长按重复使用本机事件。

AppKit 回归通过：缺失/迟到抬键、重复 Command+A、全选后触发未支持的 Command+X、多修饰键导航、Control 导航原有按下/释放和失焦清理。原有中文预编辑/提交、主动双向剪贴板、自动分辨率与关闭断开回归也通过。Swift release、Vue 类型检查与生产构建、Tauri macOS 应用包构建通过。

Ubuntu 另使用临时 HOME、自建 Xvfb 与 GTK 编辑器，对照发送旧版漏掉 A 释放的序列，实际重现连续 `a`；完整全选快捷键后等待 1.2 秒，内容保持不变，原生 Ctrl+X 剪切后再等待 1.2 秒也未出现额外字符。随后 Unicode 粘贴、主动复制、多行内容、无选中复制超时恢复及原有进程保留回归通过。该测试未操作用户桌面应用。Command+X 的控制端剪切回传仍未实现，Linux 夹具使用的是 GTK 自身的 Ctrl+X。

已安装新的 Mac 应用包，WindowHost SHA-256 为 `dc58b42f7c5fb72d03c920871b7bb5fd0eadb0ef4d90d654751166d7d12a991f`，旧包已备份。本次没有重启 Ubuntu 正式服务；正在运行的旧 WindowHost 需要断开并重新连接才能加载修复。尚未复现用户当时的完整页面操作序列。

## 2026-10-21 图片、文件与剪切通道

按用户要求，剪贴板已扩展为主动双向文本、PNG 截图、普通文件及文件夹，并补齐 Command+X 文本/图片原生剪切。格式、协议、大小和缓存规则见[剪贴板说明](clipboard.md)。二进制内容使用自有 SSH channel 的长度受限 manifest + 原始字节流；X11 两个方向支持 INCR，没有引入 Xpra 或后台同步。

最终根 workspace 测试 **36 项通过、0 失败、4 项 Linux 专用忽略**，格式、Clippy 与新增 Python 脚本语法检查通过。Mac 原生 release、Vue 类型检查/生产构建、Tauri 完整应用包构建通过。

- `controller_input_smoke.py` 使用独立命名 AppKit 粘贴板验证 PNG/TIFF 转换后的完整尺寸、文件/文件夹 URL、二进制接收、路径校验、Command+X 和重复键防护；原有中文预编辑、分辨率及关闭断开回归通过。访问粘贴板服务需要允许相应 macOS IPC；未读写用户系统剪贴板。
- `clipboard_ordering_smoke.py` 使用实际 AppDelegate 与独立粘贴板验证：剪切后立即粘贴使用新内容，无关回包被拒绝；复制失败时取消对应排队粘贴；等待期间较新的本机复制不会被旧回包覆盖。
- `mac_clipboard_bulk_smoke.py` 使用真实 Mac ↔ Ubuntu SSH，生成的超过 1 MiB PNG、中文文件名、文件夹及空文件往返字节一致，本轮远端快照已清理。
- Ubuntu `rich_clipboard_smoke.py` 在自有 Xvfb/GTK、私有 HOME 中通过超过 1 MiB PNG 的双向 INCR、文件/文件夹/空文件内容哈希、文件剪切提前拒绝、路径穿越及中断流拒绝。
- `existing_window_smoke.py --controller-text` 通过文本原生剪切、空/超限内容在删除前失败、超时后继续输入、终端剪切拒绝，以及原有进程存活检查。夹具新增滚动容器，以免超长文本把测试窗口撑到渲染预算之外而导致 `window_unavailable`。
- `browser_text_smoke.py` 的独立 Firefox 网页收到真正的 PNG 和文件对象，SHA-256 与源内容一致；8 轮中文复制/剪切/粘贴/跳转、新建和关闭标签、40 次 Unicode 提交、断开重连与实际 H.264 帧均通过。

Mac 新包已安装，WindowHost SHA-256 为 `4e0a8b85f2440703661484ddde4b03707b70b96d5f847e46ea67a5c286990955`。Ubuntu 二进制已备份并原子更新，最终 SHA-256 为 `58a29faf12d1402e440627b079e5d355c11924555ccca58597ef5b674a7ddf39`。经用户确认后已重启正式 Agent 并重开独立“设置”；服务 active，运行中二进制哈希与安装版本一致，握手已启用 `clipboard.bundle` / `clipboard.cut`，状态目录与 socket 保持 0700 / 0600。“设置”会话和一个 X11 窗口仍可用；原桌面 ChatGPT 的 PID、启动时间与可执行文件身份在重启前后保持一致。需在启动器重新读取应用与窗口并重新连接，以加载新宿主和会话目录。

安装后的默认 Agent 另通过真实 SSH 通道上传、下载 65,537 字节生成文件，内容完全一致，随后清理该测试快照。最终补齐桥接粘贴必须等待进行中的 INCR 结束才释放后续输入的检查，格式、Clippy 与隔离 GTK 大图片双向 INCR 回归通过；没有据此宣称真实 Wayland 桥接已完成端到端验收。

原桌面 ChatGPT 未进行输入、截图或附件操作。上述验证覆盖自有 GTK、独立 Firefox 页面及真实 SSH 字节流，不能代替所有编辑器、文件管理器、默认 ChatGPT 附件界面或长期性能验收。未重新测量输入到屏幕的整体延迟。跨端文件移动、符号链接和特殊文件不在本次支持范围。

## 2026-10-21 应用缓存与统一连接入口

启动器按 SSH 主机、账号和端口缓存应用名称、图标及自定义入口，重开直接恢复，自动读取窗口与会话状态；仅“刷新应用”重新扫描目录。刷新失败或发现组件不可用时保留已成功读取的应用列表，缓存损坏或存储不可用时可退回内存使用并提示。运行列表合并原桌面窗口和远程会话，统一“连接”按已有 session ID 恢复或接管新窗口；同一窗口去重，同应用独立实例保留。返回启动器时在后台刷新运行状态，重叠的聚焦读取合并，较旧的回包不覆盖新状态。

`npm test` **14 项通过、0 失败**，包含缓存跨启动保存、账号/端口隔离、失败保留、窗口与会话去重及独立实例。`launcher-flow.test.mjs` 编译并执行实际 `App.vue` setup 与 Vue mount/unmount 生命周期，使用非 DOM renderer，只替换 Tauri IPC、存储和聚焦来源；验证重开及聚焦不调用 `list_apps`、主动刷新才调用、统一入口正确调用恢复/接管、已连接时不重复操作、恢复失败不另开应用、发现失败不阻塞已有会话，以及旧主机迟到回包隔离。Vue 类型检查与 Vite 生产构建通过；这些测试不等同于真实 WebView / SSH 界面的端到端验收。本轮只修改启动器，Ubuntu Agent 与视频/输入通道未变。

Swift release 与 Tauri 完整 macOS 应用包构建通过，新包已备份替换到原工作区安装路径。启动器 SHA-256 为 `87b0059ceae2a76af3ea86cba41a88a4f6e0f7bd5c4a28d84d7e439c46bc22f8`；内嵌 WindowHost 仍为上轮 `4e0a8b85f2440703661484ddde4b03707b70b96d5f847e46ea67a5c286990955`。正在运行的启动器需退出并重开以加载新版；本轮没有重启 Ubuntu Agent 或接管用户桌面窗口。

随后按用户要求重开实际 macOS 启动器并联通真实 Ubuntu Agent：首次自动读取 48 个应用，第二次退出重开显示“已使用缓存 48 个已安装应用”，更新时间仍为 22:21:21；点击“刷新运行状态”不改变该时间，点击“刷新应用”后变为 22:22:03，图标正常显示。统一列表仅显示一个已接管的 ChatGPT 条目（只读元数据），未操作其宿主或内容。点击“设置”的“连接”恢复原 session ID，实际呈现 2940×1604 / 200% / H.264；关闭本机宿主后同一远端会话继续存在。自建临时 GTK 桌面窗口经同一入口完成首次接管，实际呈现 H.264，连接后列表保持单行并显示“已连接”。原桌面临时窗口的键鼠未生效，独立“设置”的搜索点击、按键和文字有响应；未据此宣称真实 Wayland 原桌面输入已通过，也未直接改变其系统权限。

GUI 中另发现返回启动器时后台刷新与“连接”共用 busy，导致第一次点击可能被吞掉、第二次才启动宿主。新增实际 App.vue 生命周期回归在旧代码上明确失败；修复使聚焦读取不占用连接 busy，并按读取版本丢弃旧状态。更新后前端 **15 项通过、0 失败**，类型检查与生产构建通过。

修复版再次完成 Swift release 与 Tauri macOS 打包、备份安装和实际重开，启动器 SHA-256 为 `f86126d90923c7cc24376568bd9e1ee8d593fe36511b50e59232d8f971710934`。GUI 复测中，“设置”一次点击恢复原 session；关闭后返回启动器，再一次点击临时窗口即恢复其原 session，并实际呈现 H.264。刷新时临时窗口始终只有一行，状态为禁用的“已连接”；较早的状态读取不会另开会话。最后关闭两个本机测试宿主，终止并清理本轮自建 GTK 进程和会话；原有“设置”与 ChatGPT 的两个 session ID 均保留且无控制端，ChatGPT 进程身份不变，Agent PID 不变。启动器最终再次重开并保持打开，自动显示缓存的 48 个应用、原更新时间 22:22:03，以及 3 个运行条目（“设置”和原桌面两个窗口），没有临时窗口或本次测试连接记录。原桌面临时窗口的输入异常仍未定位最终原因，未将该部分记为通过。

## 2026-10-21 断网后的 SSH 登录恢复

本次实机排查时，10.0.1.66 的 SSH 服务可达，但项目 ControlMaster socket 已不存在，严格校验下的 BatchMode 连接返回 `Permission denied (publickey,password)`。使用原有账号重新认证后，Agent 服务正常，原“设置”、ChatGPT 与用户当前受控应用的 session ID 均保留；当前应用仍有控制端。说明本次列表失败来自临时 SSH 认证丢失，不能据此判断 Agent 已停止。没有重启 Agent 或操作原桌面 ChatGPT 内容。

SSH 传输现区分认证失效、网络失败、主机指纹校验失败和 Agent 未运行，使用固定错误文案并限制 stderr 读取。新连接启用 10 秒间隔、连续 2 次无响应检测；登录辅助脚本验证真实认证通道，健康主连接直接复用，只有用户显式重新登录才替换无响应的项目主连接，`--check` 保持只读。不会自动接受未知指纹、配置密钥或延长原有 10 分钟空闲认证时间。

启动器增加“重新登录 SSH”，通过经过校验的主机、用户和端口生成私有 0700 `.command` 文档，打开系统终端中的固定脚本；密码由系统 SSH 接收，不写入配置或登录文档。返回启动器和网络恢复事件触发只读元数据刷新，成功后清除旧连接错误；旧 TCP 请求不会阻塞恢复或覆盖新状态。应用缓存保留，不自动重放应用启动、接管、恢复连接或输入操作。

根 workspace 测试 **37 项通过、0 失败、4 项 Linux 专用忽略**，格式与 Clippy 通过。前端 **17 项通过**、Vue 类型检查与生产构建通过；新增实际 App.vue 生命周期测试覆盖断线、认证失效、显式登录、聚焦/在线恢复、卡住的旧请求与不重放应用操作。Tauri 原生 **2 项测试通过**，格式与 Clippy 通过，验证端点拒绝注入、固定脚本与私有权限。SSH 辅助脚本 11 项非法参数检查、7 项恢复夹具场景通过，包含健康复用、认证丢失、无响应主连接、只读检查及探测超时；当前真实认证通道的 `--check` 也通过。夹具模拟断线，不等于实际切换 Wi-Fi 或拔网线验收；保持用户现有应用连接，未对其主连接进行破坏性断线测试。

Swift release 与 Tauri 完整 Mac 应用包构建通过，已备份安装并仅重开启动器。安装后的 launcher SHA-256 为 `4a18de4897f4b24c76bb2a2ce3a39fdccb598c2619473d95a85f3f4682162aee`，WindowHost 保持 `4e0a8b85f2440703661484ddde4b03707b70b96d5f847e46ea67a5c286990955`。实际 GUI 恢复缓存 48 个应用、原更新时间 22:22:03 与 4 个统一运行条目；点击“重新登录 SSH”后原生命令成功返回，生成的登录文档及目录均为 0700、端点与界面一致。点击“刷新运行状态”后显示“SSH 登录已恢复，窗口与会话列表已更新。”，无连接错误，缓存时间不变。界面工具禁止读取 Terminal，未将终端内输入密码和返回聚焦记作本轮 GUI 全流程通过；真实重新认证与辅助脚本复用另由 SSH 检查确认。

安装后再次只读核验：Agent 仍为 active、PID 1095794；三个原 session ID 与原受控状态均不变；原桌面 ChatGPT 的 PID、启动时间和可执行文件身份不变。未重启 Ubuntu Agent 或关闭用户正在使用的宿主。

## 2026-10-21 账号密码自动认证与原窗口恢复

本节记录中间版本。用户随后明确要求保存在 Mac 服务中、不使用钥匙串，当前实现与验收以之后的“Mac 服务连接配置”和“Ubuntu 应用图标”两节为准。

启动器直接接受 SSH 主机、账号、密码和端口；密码通过原生帮助进程的匿名 stdin 保存到 Mac 系统钥匙串，按端点隔离，普通连接配置及应用缓存不保存密码。网络中断后由系统 SSH 的 askpass 机制读取该端点密码，视频、输入、元数据和剪贴板统一使用这个认证路径。没有生成 SSH 密钥或新增 Ubuntu 公钥授权；实机 `authorized_keys` 仍为空文件，Agent PID 1095794 和三个原 session ID 均保留。

原窗口按 1、2、4、8、15 秒退避重连，网络恢复事件可提前触发。恢复使用同一个 session ID，保留本机 NSWindow、大小及固定/自动分辨率模式；视频解码器与序号按新连接重建。断线时取消排队输入、预编辑及剪贴板任务，不自动重放这些操作；退出取消重连。尚未收到首次启动回包时复用原 request ID，已知会话结束不另开应用。启动器只读后台重试元数据，保留应用缓存；正常界面只保留“连接”，不要求反复手动登录 SSH。

根 workspace **38 项通过、0 失败、4 项 Linux 专用忽略**，格式和 Clippy 通过；前端 **19 项通过**、类型检查及生产构建通过；Tauri 原生 Clippy、Swift release 与完整 Mac 应用打包通过。`automatic_reconnect_smoke.py` 用真实 AppDelegate/SSHTransport 与私有子进程故意断开第一次传输，验证自动恢复相同 NSWindow/session、低序号新快照、尺寸模式保留、断线输入丢弃、退出取消与幂等启动。`ssh_password_smoke.py` 使用临时独立钥匙串验证保存/更新、端点隔离、密码提示限制与拒绝直接 CLI 导出；系统已有密码项未用于该夹具。控制端输入、PNG/文件剪贴板、剪切顺序、自动分辨率及关闭断开回归通过。夹具不是实际 Wi-Fi 切换验收。

实机密码保存操作已通过启动器安全输入框完成，输入随后清空，钥匙串项存在。首次真实自动连接暴露 OpenSSH 创建复用 socket 时的临时随机后缀将路径扩至 macOS `sockaddr_un` 上限：已缩短为 `~/.ssh/lra-pw-%C`，并增加按实际参数展开的长度回归。最终安装 launcher SHA-256 为 `bad7a378b308256651044762d1141bfd82b5b619b9c6991f92a85d876a575b09`，WindowHost 为 `ae75bf27cb5a5e6bc5efb383badc6eefae161e22de77033c0b947cba014df66f`，旧包均已备份。

更新帮助进程后，macOS 系统钥匙串请求应用访问授权；界面工具明确禁止操作 `com.apple.SecurityAgent`，已请用户在 Mac 处理该系统弹窗。当前未将新版真实自动重新认证及真实 SSH 断开后画面恢复记为通过；仍待该授权后的实机验证。开发包未签名公证，二进制更新可能重新触发钥匙串访问确认；不能把本机密码项存在、隔离夹具通过或后台持续重试当作真实自动认证通过。

## 2026-10-21～22 Mac 服务连接配置

按用户“Mac 服务，就是我填什么就用什么”的要求，连接账号密码改为由 Tauri 后端写入自己的 `Application Support/io.lanremoteapp.launcher/ssh-connections.json`。这是本机私有 JSON 配置，目录 0700、文件 0600，按主机、用户和端口隔离，使用文件锁及原子替换保存；不保存到浏览器偏好、命令行或日志。Swift askpass 与启动器传输读取同一配置，不使用系统钥匙串、不生成 SSH 密钥。本软件此前创建的单个钥匙串密码项已删除，没有读取其内容或操作其他项目。

Tauri 原生 **5 项测试通过**，包含端点隔离、更新、私有权限、损坏与符号链接拒绝及 8 个并发写入保留所有端点；格式与 Clippy 通过。`ssh_password_smoke.py` 在私有服务配置中验证更新、文件权限、版本/重复端点/长度/链接校验、仅密码提示响应和拒绝直接 CLI 导出，未访问系统钥匙串。原生自动重连与剪贴板顺序夹具通过。图标改动后的最终前端 **20 项通过**，Vue 类型检查、生产构建及完整 Mac 应用打包通过。

实机在旧临时登录主连接已不存在时，由启动器安全输入框提交原有账号密码，成功建立新的 SSH 密码认证主连接 PID 49145，自动读取运行列表并恢复原“设置”会话；没有钥匙串弹窗。多次退出并重开启动器均直接恢复缓存的 48 个应用，更新时间仍为 2026/10/21 22:22:03，密码框留空也可连接。私有配置实际权限符合 0700/0600，匹配端点仅一条。

最终联调只向本次“设置”宿主的 SSH 子进程 PID 51517 发送 SIGTERM；同一宿主 PID 51511 自动建立新子进程 PID 51640，恢复同一 session ID，保持 2940×1610 / 200% / H.264。恢复后点击搜索并输入 `display`，实际视频呈现搜索文字和结果，再清空搜索；证明收到新画面和输入生效。测试没有终止公共主连接，PID 49145 保持不变；这轮断线注入验证的是 SSH 子通道恢复，首次密码重新认证另由上述真实登录证明，不能合称拔网线、切换 Wi-Fi 或共享主连接丢失后的完整实机验收。

结束时只退出本次“设置”宿主，同一远端会话继续存在且无控制端。Agent 始终 active、PID 1095794；原 ChatGPT 的 PID 437845、启动时间 13451824 与可执行文件身份保持一致，没有读取画面或发送输入。用户原有受控会话 `ed03455af109cee8c9fbf8e3867d061d` 保持受控；联调期间另出现的会话没有被本轮操作或清理。Ubuntu `authorized_keys` 仍为空文件，没有重启或升级 Agent。

## 2026-10-22 Ubuntu 应用图标

启动器把所选应用缓存或运行窗口中的 PNG 图标，通过本机匿名 stdin 管道一次性交给 WindowHost。新建、接管和恢复会话共用此路径，不额外扫描 Ubuntu 应用，不写临时图标文件或把图像放入命令行。宿主在激活后设置自己的 `NSApplication.applicationIconImage`，Dock 和本机弹窗使用对应 Ubuntu 图标；传输重连不会清除该图标。缺失、非 PNG、超出 16 KiB 解码数据或 512×512 像素的图标退回随包 RemoteApp 标识。

实际 App.vue 测试确认已安装应用启动、首次窗口接管及已有会话恢复均传递正确图标，缓存重开不调用 `list_apps`。真实 AppKit 自动重连夹具确认分段 stdin PNG、透明度保留、非法/过大图标拒绝、应用图标实际内容以及重连后保留；检查考虑系统的 Retina 栅格化及显示色彩转换，不以对象身份或 TIFF 元数据相等替代内容验证。原有相同 NSWindow/session、分辨率模式、序号重置、断线输入丢弃、退出取消及启动幂等性回归通过。

真实启动器恢复原“设置”会话，宿主的本机弹窗实际呈现 Ubuntu 的齿轮图标。系统 Dock 的直接 UI 读取超时，未将整条 Dock 的截图检查记为通过；Dock 图标赋值及其内容由实际 AppKit 夹具验证。完成上述 SSH 子通道恢复后，原宿主未重建且搜索画面正常。本轮没有启动额外的远端测试应用或改变任何系统设置。

新包已备份安装到原工作区路径，启动器 SHA-256 为 `ceb17357ae8f78fda6ca4cd94df340fb18509b136bafe58ad03b5205272ef5e2`，内嵌 WindowHost 为 `d3501c1a6ba12780c869c4d8857fb3d1993abc9d69b09acfd3ff7f867fe739ea`。正在运行的旧宿主需断开后重新连接以加载图标功能，远端应用可继续保留。

## 2026-10-22 Dock 与应用切换器名称

启动器在新建、接管和恢复会话时交付所选 Ubuntu 应用名称。后端在私有 `Application Support/io.lanremoteapp.launcher/WindowHosts` 下缓存对应的本机 `.app` 入口，使用应用名称作为包名、`CFBundleName` 和 `CFBundleDisplayName`；各主机、账号、端口、应用 ID、名称及宿主二进制版本分别形成稳定身份。完整生成后才原子发布，不修改已安装的主包或运行中的旧宿主。该方案使用公开的[应用 Bundle 结构](https://developer.apple.com/library/archive/documentation/CoreFoundation/Conceptual/CFBundles/BundleTypes/BundleTypes.html)，未调用私有进程改名接口。窗口中的网页、文档标题继续独立更新。

前端 **20 项通过**，实际 App.vue 验证缓存应用启动、窗口接管和会话恢复均传递应用名称与图标，不增加远端扫描。Tauri 后端 **8 项通过**，覆盖 Unicode/XML 转义、路径安全、缺失名称回退、缓存复用、同名应用/端点隔离、宿主更新不覆盖旧版本及链接重定向拒绝；格式、Clippy、Vue 类型检查/生产构建和完整 macOS 打包通过。实际 AppKit 自动重连夹具验证应用菜单名称、Ubuntu 图标、相同 NSWindow/session、分辨率保留和断线输入丢弃。

安装后重开启动器，缓存的 48 个应用及用户已刷新的 2026/10/22 00:06:48 时间保持不变。通过统一入口恢复原“设置”会话，macOS UI 将宿主识别为 `App: 设置`，应用菜单显示“设置”，实际呈现 2940×1620 / 200% / H.264。退出本机测试宿主后再次连接同一会话，macOS 名称保持“设置”，复用同一入口且 Info.plist 修改时间未变。另用 `lsappinfo info` 只读核验该 bundle：LaunchServices 实际登记名称为“设置”、类型为 Foreground，宿主可执行文件仍是 RemoteAppWindowHost。未把窗口标题相同视作应用身份验证；本轮未直接读取 Dock 悬停标签或应用切换器截图。

测试最后只关闭本机“设置”宿主，远端原 session ID 保留且无控制端。Agent 仍 active、PID 1095794，用户原受控会话和其他原 session 保留；原桌面 ChatGPT 的 PID 437845、启动时间 13451824、可执行文件身份不变。未重启 Agent、终止公共 SSH 主连接、输入用户应用或读取其画面。

新版已备份安装：launcher SHA-256 `ae4b287866be8508c5a8d7dd0bfde2f2d7d5b0e7b2f93938e6f7365e97144214`，WindowHost `92e8c47c7898aaa2e8784daa53ef606e58ef9e273d2fd6ffe49531318c7359b8`；生成入口中的二进制与该宿主哈希相同。已运行的旧宿主需断开并重新连接后使用新名称，远端应用继续保留。验证使用当前未签名开发包，不能据此宣称签名公证、系统网络切换或生产环境验收通过。

## 2026-10-22：Agent 统一原桌面授权

本轮新增 Agent 共享的 input-only RemoteDesktop Portal v2 / libei 连接；Xwayland Backend 仍独立核验主窗口/进程并采集，不再逐应用创建 XTEST 门户。首次选择记住后，恢复凭据仅保存在 Ubuntu 私有状态目录，按系统单次令牌语义原子轮换。列表读取、独立 Xvfb 会话和断开释放不申请授权。

- `scripts/check.sh` 完整通过：40 个 Rust 测试通过、4 个依赖 Linux 环境的测试按原规则忽略；3 个新增授权存储/输入所有权测试通过；20 个前端测试、类型检查与构建，以及 Swift 输入、富剪贴板、复制剪切顺序、SSH 密码与自动重连 smoke 均通过。更新后的 AppKit smoke 检查授权提示的 session 隔离、画面元数据不会盖掉提示、授权恢复清除提示和等待时不弹阻塞错误框。本地 Clippy `--workspace --all-targets -- -D warnings` 通过。
- Ubuntu 工作区测试 40 项通过、4 项同样按规则忽略，release 构建通过。Ubuntu 没有安装 Clippy，本轮未安装新工具，Clippy 结果来自 Mac。
- `desktop_authorization_smoke.py` 在 Ubuntu 的专用 D-Bus（禁用服务自动激活）、独立 HOME 和真实系统 libei/libeis 上通过：多个输入租约仅申请一次、真实键盘/绝对指针/按钮/滚轮事件、按租约释放、设备移除/重新加入期间不补发输入、重启恢复和令牌轮换、撤销后重新确认、取消后打字不重复申请、portal 重启后重新注册和恢复。fixture 只实现 input-only 门户，不能把它当作 GNOME 系统弹窗实测。
- Ubuntu 的 `existing_window_smoke.py --controller-text` 与 `app_discovery_smoke.py` 再次通过，包含进程和主窗口隔离、拒绝其他窗口、中文输入、剪切/粘贴、重连复用与原进程在 Agent 重启后存活。均使用自建 Xvfb / GTK 测试程序，没有操作用户原桌面程序。
- Mac release 开发应用打包通过。首轮实机接入发现 SSHTransport 的消息白名单漏掉新增授权事件，读取器主动断开；已在 `857890e` 修复，并让自动重连 fixture 从真实 SSH 子进程发送授权事件，覆盖读取器、序列号和 AppDelegate。修复后的针对性 smoke 与 release 打包均通过，重新安装后连接正常。

用户明确批准重启 Agent，并知悉这会结束旧的“设置”和 EasyTier GUI 自有会话。Ubuntu Agent 已备份升级为 SHA-256 `37df361d710e3ddb745a58934e56d3fd8946d532c4038b264b4430e64c217ae9`；Mac 内嵌宿主已升级为 `dcf74f48ea7ca51526a710d47e71db5d88242901c4e083b2473df729b244ad88`。启动器仍使用原来的 48 个缓存应用，未重读目录或改变 SSH 认证配置。

真实 GNOME / Xwayland 测试使用两个专用 GTK 进程（A / B），不接入用户原桌面应用内容：

- 首次请求在等待期间曾超过三分钟时限，系统弹窗已关闭；重新连接后，系统返回了可恢复授权，状态文件 0600。控制端从等待状态进入 H.264 就绪，A 的鼠标点击到达远端，计数从 0 到 1。自动化工具直接提交中文/粘贴未能可靠交付，本轮没有据此宣称中文自动化通过；用户使用 Mac 的真实输入法确认中文正常上屏。
- 成功授权后再启动全新的 B 进程并连接，Agent 1173799 仍共用原授权进程 1174363，恢复凭据未重写。电脑操作工具拒绝控制新 Mac 宿主 LanRemoteAppAuthB，未绕过限制；用户手动验证 B 点击计数变为 1 并关闭本机 B 窗口，远端测试进程保留。
- 从本机退出 A，再从统一入口连接，仍是 session `e7866ebf53e7ddad15f4281cbb89164c`，中文内容保留、授权直接就绪；点击计数从 1 到 2，凭据与授权进程均未变化。之后再用红色关闭按钮断开，两个远端进程仍存活。
- 两个测试控制端都退出后，真正重启 Agent。旧授权进程已结束，新的 Agent 为 1179418，新的授权进程为 1179519（核对所有线程的 children，不只主线程）。重新接入 A 生成新 session `72705f27bd7499b9ac9bef9d6f18fd9f`，无需再次确认，自动进入就绪；点击计数从 2 到 3。凭据文件在此次恢复后原子重写，权限保持 0600。此版本真实 portal 返回了相同的 opaque token 字符串，恢复判断依据是新进程的就绪状态和真实输入到达，不以 token 字符串变化作为成功条件。

测试结束已关闭本机 A 连接，并按测试脚本身份核验后清理两个专用 GTK 进程；Agent 保持 active（1179418）、活动会话归零、恢复授权保留。原桌面 ChatGPT PID 437845 与飞书 PID 44108 仍存活，可执行文件身份保持；本轮没有输入、缩放或读取这两个应用画面。未重启 Ubuntu 系统，未宣称系统重启、其他桌面后端、真实 portal 服务重启、长期稳定性或生产环境验收已通过。

## 2026-10-22：统一产品与 Mac → Ubuntu 组件

新增 Mac 窗口发布组件、Ubuntu GTK 管理与接收窗口、同产品安装入口、设备 ID 与会话归属模型，以及按 SSH 端点独立管理的 StreamLocal 返回通道。既有 Ubuntu → Mac 的宿主与认证配置保留。使用步骤和首版限制见 [Mac 发布指南](mac-publishing.md)。

已完成的本地与隔离验证：

- 最终固定代码后，`scripts/check.sh` 完整通过：Rust 42 项通过、4 项按既有 Linux 条件忽略；3 项 Python 授权单测、SSH 辅助脚本与恢复验证、20 项前端测试、类型/构建，以及 Mac 两个 Swift 组件编译、发布边界、原宿主读取/输入/富剪贴板/顺序/自动重连/密码配置 smoke 均通过。Ubuntu Rust 同样 42 项通过、4 项忽略。首次完整检查因同时修改 Swift 输入文件而中断，已固定代码重跑，未把中断结果当作通过。
- Mac Swift 发布组件 release 编译与开发包构建通过；独立边界 smoke 验证接收应用包/缓存入口及系统安全界面排除、标识符限制、视频与图片像素上限。它不申请录屏或辅助功能，也不操作用户窗口。
- Mac 启动器 20 项前端测试、类型检查、Vite 构建，以及 8 项 Rust 后端测试通过；启动器和 Rust 工作区的 Clippy `-D warnings` 通过。新增专用返回连接参数测试，验证不更改共享 SSH master、不清除所需返回转发、不接受注入地址。
- Ubuntu `node_routes_smoke.py` 在独立 HOME 的真实 Agent 上通过：设备身份和停止共享状态跨服务重启保留，两个设备的记录互相隔离，同设备旧代连接结束不清理新代，非法设备 ID 被拒绝，停止共享拒绝目录访问。
- Ubuntu `linux_receiver_smoke.py` 在独立 Xvfb/HOME、真实 GTK/GStreamer 和协议 fixture 上通过：实际 H.264 解码与 GTK 像素显示、SPS 尺寸拒绝、GTK 输入上下文中文提交、按键/鼠标消息、原生文本/PNG/文件剪贴板读取写入、目录与文件大小检查、恢复原会话且不重放输入/启动。发现并修复 PyGObject 文件选择区注册 API 差异。这里的服务端是测试 fixture，不能当作真实 ScreenCaptureKit 或 Ubuntu 中文候选框验收。
- 原链路 `native_session_smoke.py` 经真实 SSH 完成 3 次重连，启动幂等、单控制端、键鼠与按住释放、缩放、隐藏恢复、会话发现和程序退出均通过。Ubuntu 的 `existing_window_smoke.py --controller-text` 与 `app_discovery_smoke.py` 通过，覆盖主窗口/进程归属、中文、原生剪切、复制粘贴和原进程在 Agent 重启后存活。

部署与服务验证：

- Mac 新版管理应用已打包并实际打开，显示统一“本机共享”和已保存设备选择。真实 Mac 服务的私有管理/发布 socket 均为 0600；连续连接后直接关闭的健康探测不再终止服务。发布 socket 拒绝远端修改共享开关和触发系统权限。初版服务曾因断开的健康探测触发 SIGPIPE 退出，已修复并实测其保持运行。
- Ubuntu 已备份并升级原 Agent、安装同产品 GTK 入口。在无活动应用会话时，将原 `systemd-run` 临时服务替换为 `~/.config/systemd/user/remoteapp-agent.service`，已 enable 且 active；原桌面 ChatGPT PID 437845 与飞书 PID 44108 仍存活。GTK 管理应用通过图形用户环境实际启动并保持 active。系统重启后的自动启动仍未实测。

最终开发产物 SHA-256：Mac launcher `05bc17642a16d26b40917a1f60b82a52444e3c054e40e2078583a7e74966a798`，Mac 发布辅助组件 `f1d3f72d1c29916deeb7469970d754ce486eb1c0ca15e607d44ebe5fac51b7fd`（开发资源与打包应用内副本一致），原 Mac 接收宿主仍为 `dcf74f48ea7ca51526a710d47e71db5d88242901c4e083b2473df729b244ad88`；Ubuntu Agent 的已安装文件与最终 release 产物均为 `44016c817913a94b3749d6e9a9df5629a7ccc75f88b33df6bd00de5b06957a48`。

用户已明确允许向 `chenrg@10.0.1.66` 共享本机应用目录、所选窗口画面、输入及主动剪贴板，并在系统设置开启两项权限。组件改由 LaunchServices 启动，使权限归属到实际共享组件。开发包临时签名更新曾导致系统保留旧授权，用户移除旧条目、重新添加当前组件后，真实服务的录屏与辅助功能检测均为 true，共享开启。没有修改系统权限数据库或借用其他程序的权限；开发包升级的授权限制见 [Mac 发布指南](mac-publishing.md)。

真实 Mac → Ubuntu 的画面部分已通过：`mac_to_linux_live.py --capture-only` 只匹配专用 `MacPublishingFixture` 窗口，在 Ubuntu 原图形用户环境运行实际 GTK 接收端，收到 SCK → VideoToolbox H.264 → SSH 返回通道的 1600 × 1184 解码图像，本次从接收端启动到首帧约 0.255 秒（不代表输入到显示延迟）。主动中断接收 socket 后，确认同会话恢复且收到新的解码图像；关闭接收窗口后原 Mac 测试窗口仍在。此模式不发送输入，也不会开启共享或授予权限。测试应用须通过 LaunchServices 启动，直接执行其二进制没有 NSRunningApplication 启动时间，无法通过窗口身份校验。

随后完整 `mac_to_linux_live.py` 在原桌面 GTK 接收窗口通过，使用上述固定发布组件，无需再次更新其签名或授权：

- 接收并显示真实 1600 × 1184 H.264 像素，启动到首帧 0.287 秒；这不是输入到显示延迟。
- Ubuntu 指针点击 Mac 原生按钮，计数恰好从 0 到 1；点击原生文本框后，GTK 输入上下文提交 `反向链路中文🙂 Mac → Ubuntu`，经 Mac 原生复制完整回传。
- Ubuntu 文本主动粘贴到 Mac，Mac 原生复制和剪切回传到 Ubuntu；剪切后再次输入 `剪切后输入正常🙂`，确认内容已删除且按键正确释放。
- 改为 1200 × 840 后收到相同尺寸的新解码像素；中断接收 socket 后同一 session 恢复，并收到新的解码图像。关闭 Ubuntu 接收窗口只释放控制权，Mac 原应用仍在。

鼠标排查中曾出现用户真实点击也无效的现象。最终日志定位为测试 fixture 的事件监控错误地读取鼠标事件的 `keyCode`，引发 AppKit 异常，中断事件分发；修复为只对键盘事件读取该字段后，坐标点击和真实双机点击均通过。该错误造成的早先“没有鼠标事件”记录不能用于判断发布组件的投递兼容性。修复只更新专用测试程序，未再次改动已授权的发布二进制。

本轮中文通过的是 GTK 输入上下文的程序化 commit 及原生文本回读，尚未验证 Ubuntu 物理输入法候选选择。反向 PNG、文件剪贴板已通过 GTK 接收组件的隔离测试，本轮双机验收覆盖文本，不能扩展为截图和文件已完成双机原生应用验收。原 Ubuntu ChatGPT、飞书的进程身份再次核对仍在，Agent 和 GTK 管理服务均 active。

并行链路验证通过：Ubuntu 原桌面 GTK 连续接收上述 Mac H.264 窗口 40 秒，期间在 Mac 运行 `native_session_smoke.py --reconnects 3 --detached-seconds 1`。正向真实 SSH/X11 的专用 xterm 完成键鼠、缩放、隐藏恢复、按住输入释放及 3 次断开恢复，反向接收保持同一连接代次且始终 ready。它验证了两个方向的传输并存，不能代替两个原生宿主同时交互或两个方向同时使用剪贴板的验收。

双向原生宿主同时交互、Ubuntu 物理输入法候选选择、物理断网、多台远端并发、复杂弹窗与长期稳定性仍未完成真实验收。

## 2026-10-22：统一传输、配对与 QUIC

两端加入公共 `remoteapp-network`，管理入口和原生接收器显式选择 QUIC 或 SSH；没有按客户端/服务端拆成两份产品。控制、每窗口可靠视频和文件传输分别使用 QUIC 流，设备证书、配对、撤销、地址和路由按 UUID 隔离。实现与使用边界见[统一传输指南](native-transport.md)。

构建和隔离检查：

- Mac 与 Ubuntu 根工作区各 **49 项通过、4 项 Linux 环境测试按原规则忽略**。Tauri 后端 8 项、前端 21 项通过，类型检查和生产构建通过；Rust 格式检查及网络组件 Clippy `-D warnings` 通过。Mac 的 `scripts/check.sh`、Swift 发布/接收编译、输入、剪贴板、SSH 认证和自动恢复回归通过；新增 Linux QUIC 接收测试另在 Ubuntu 执行通过。
- 三个真实本机 QUIC 节点验证双向调用、一次性邀请复用拒绝、证书不匹配拒绝、撤销一台设备不影响另一台，以及远端不得改变共享开关、权限或创建 SSH 返回路由。媒体测试验证元数据先于帧、H.264 顺序、失效几何/销毁窗口帧过滤。这里只模拟三台设备，不能称为三台物理设备验收。
- Python 生产 `Wire` 的 SSH/QUIC 路由隔离通过：同 UUID 可有两个显式入口，撤销 QUIC 不会自动退回 SSH。关闭连接时，即使父进程保持 stdin 打开，桥接子进程也会退出并向接收宿主交付 EOF。
- AppKit 的实际接收器经两个本机 QUIC 节点解码合成的真实 H.264 关键帧与 7 个依赖 P 帧；重连后核验红色像素、同窗口/同会话、丢弃离线输入且不重发启动。Swift 原生剪贴板适配器经独立二进制流往返大于 1 MiB 的 PNG、Unicode 文件名、目录与空文件。使用独立测试粘贴板和发布夹具。
- Ubuntu GTK/GStreamer 接收器分别经兼容 socket 和真实 QUIC 节点通过实际像素显示、H.264/SPS 边界、中文 commit、键鼠、文本/PNG/文件剪贴板与断线恢复。独立真实 Agent 的 H.264 测试经 QUIC 通过关键帧/P 帧、视频期间输入、641×483 奇数缩放、动态/静止重连关键帧；导出的 17 个 AU 在 Mac 全部硬件解码，核验响应像素、尺寸和非法 NAL 拒绝。

真实两台机器的传输验收：

- 使用任务私有目录、临时配对和两个新网络进程连接 `10.0.1.66`；现有 Agent 与已授权的 Mac 发布组件继续运行。临时邀请仅经已验证的 SSH 管理管道交给测试程序，业务 RWP/画面/输入走 QUIC，没有 SSH 回退。
- Ubuntu → Mac：SSH 与 QUIC 各运行一遍真实 X11/xterm 测试，均通过启动幂等、单控制端、实际键鼠、按住输入断开释放、PNG 尺寸、缩放、隐藏/重显、会话发现、3 次重连及关闭测试程序。SSH 在 QUIC 测试中仅用于检查测试标记和 X11 状态，不承载被测业务通道。
- Mac → Ubuntu：用户恢复本机共享后，QUIC 和 SSH 返回通道分别通过专用 Mac 测试窗口的真实 SCK → VideoToolbox H.264 → Ubuntu GTK 解码、点击恰好一次、中文/emoji 提交、双向文本复制/粘贴、剪切后删除和按键释放、1200×840 缩放、原会话重连及断开保留 Mac 应用。Ubuntu 接收端运行于独立 Xvfb；中文为 GTK 输入上下文的程序化 commit，不是物理输入法候选选择。
- 双向并行：Ubuntu 通过 QUIC 连续接收 Mac H.264 窗口 30 秒，同时 Mac 经同一对 QUIC 节点完成正向真实 xterm 键鼠、缩放、隐藏及 3 次重连。反向接收保持相同连接代次和 ready，随后自身重连也收到新解码帧。没有把它扩大为两边同时操作系统剪贴板或两边物理键盘同时交互。

Mac release 开发包及 Ubuntu 包构建通过。为保留正在运行的应用包和权限归属，Mac 使用独立 Cargo 输出目录，并核验新包确实包含发布器、接收器和网络组件。开发产物在 `dist/lan-remoteapp-quic-macos/LAN RemoteApp.app` 与 `dist/lan-remoteapp-linux-quic.tar.gz`；本轮未运行升级安装脚本。Mac 发布器二进制 SHA-256 仍为 `f1d3f72d1c29916deeb7469970d754ce486eb1c0ca15e607d44ebe5fac51b7fd`。新 launcher 为 `d1fe404d2a7f891f277d91e3426d402fcc74f7eaa43f399e9079aa573ce20314`、WindowHost 为 `5b7cb9eb6c23fda538b3f4dab890d479591ced0d0424ef5d8964b387971e4673`、网络组件为 `5166ac545d6acb42749e0b9193e55c76f847b0eb4671b33fa83ff0c496a89ff5`；Linux 压缩包 SHA-256 为 `35de78f1b0dea7fd5308f07819fcfb4651a4657ed50c6e95e9b7c9701af92520`。这些是本机开发构建，不能称为签名公证、CI 或生产验收。

性能对照追加：六轮共 540 个有效样本，动态窗口 SSH / QUIC 中位数为 93.01 / 113.83ms、P95 为 147.19 / 213.38ms；静止后输入中位数为 83.46 / 67.86ms。当前路径为 utun4，不能泛化为全部局域网。两种通道各 20 次逻辑会话恢复通过，物理断网/休眠与长期稳定性未测。完整方法、全部轮次和原始数据见 [SSH 与 QUIC 实机对照](performance/2026-10-22-ssh-quic.md)。

## 2026-10-22：QUIC 独立双向链路与产品 SSH 移除

删除前先完成 QUIC 的双向基线验证，随后移除公共传输层、Mac 启动器/WindowHost、Linux 返回路由中的 SSH，以及账号密码、协议选择和对应配置入口。新版仍为每个平台一个产品，同时发布和接收；设备配对、缓存、窗口和重连按 UUID 隔离。实现提交 `de1afb0`，真实 AppKit 夹具 `0cf7df3`，Linux 历史参数拒绝修复 `00d0248`。

删除后的双机业务测试关闭了任务专用 SSH master：Ubuntu GTK 编辑器经 QUIC 在真实 AppKit 中显示 H.264，中文、文本剪贴板、缩放、关闭及每轮 3 次原窗口/原会话恢复通过；Mac 原生窗口经 QUIC 在 Ubuntu GTK 中完成画面、点击、中文、文本剪贴板、缩放和恢复。双向并行保持 60 秒，正向交互及重连期间反向画面连接保持就绪。三个本机节点验证双向请求、多个设备独立路由、撤销隔离及拒绝旧返回路径。

Mac 完整 `scripts/check.sh` 通过：Rust 44 项通过、4 项忽略，前端 22 项及 Swift/AppKit 回归通过；Tauri 后端 5 项、工作区及 Tauri Clippy 通过。Ubuntu 44 项 Rust 测试、Python、三节点路由、GTK/QUIC 和 release 构建通过；因没有 rustfmt/Node，未将 Ubuntu 的完整脚本记为通过。两端新开发包已构建，未覆盖仍有用户会话的旧安装。测试条件、失败后复测、复现命令、产物哈希与未覆盖项见[独立验收记录](quic-only-validation.md)。本轮不构成性能提升、CI 或生产验收。

## 2026-10-22：两端正式安装升级至 QUIC-only

用户明确允许升级两端、重启服务和结束当前会话。本轮将上一轮已验收的 QUIC-only 开发包实际安装到 Mac 原应用路径和 Ubuntu 用户安装目录，先备份旧二进制/安装文件，再停止旧产品进程并启动新版。Mac launcher、WindowHost、发布器和网络组件，以及 Ubuntu Agent、网络组件、GTK 脚本均与构建产物哈希一致。Mac 发布器代码未变化，原共享开关、录屏和辅助功能授权继续有效；新版启动器已清理本应用的旧 SSH 密码文件，原 SSH 返回进程退出。

两端正式网络状态目录首次完成持久 QUIC 配对，使用已有设备 UUID；Mac 连接 Ubuntu `10.0.1.66:42424`，Ubuntu 保存观察到的 Mac 地址 `10.0.1.253:42424`。设备邀请仅经已验证的部署管道传递，未写入验收记录。一个产品继续同时发布和接收，无需另装另一个角色。

| 检查 | 实际结果 |
| --- | --- |
| Ubuntu → Mac | 正式 Ubuntu Agent、两端正式 QUIC 组件和真实 AppKit：H.264、中文/emoji、文本复制/粘贴/剪切、960×640 缩放、3 次同窗口/同会话自动恢复和关闭测试应用通过 |
| Mac → Ubuntu | 正式 Mac 发布器、两端正式 QUIC 和已安装 GTK/GStreamer：真实 SCK/H.264、点击、中文、文本剪贴板、剪切后删除、按键释放、1200×840 缩放、同会话恢复及断开保留应用通过 |
| 实际 Mac 安装包 | 新管理器显示“QUIC 设备连接”和 Ubuntu 已连接；读取 49 个已安装应用（含本次测试入口）。从管理器打开专用编辑器，实际打包宿主显示 2940×1610、200%、H.264，中文和 emoji 在画面及远端测试缓冲中均确认 |
| 网络组件重启 | 两端 QUIC 网络进程分别重启，设备 UUID、证书指纹和配对记录不变；Mac 管理器自动重新启动本机网络组件，原打包宿主继续显示原内容，新增输入到达远端并更新画面 |
| 重启后的反向链路 | 已安装 Ubuntu GTK 接收器重新收到真实 1200×840 H.264，之后同会话重连收到新解码帧 |
| 收尾 | 本次测试应用、临时桌面入口和部署文件清理，清理检查时 Ubuntu 远程会话为 0；正式 Agent 和 GTK 管理服务保持 active，Mac 新版管理器/发布器/网络组件保持运行，永久配对保留。用户随后新开的窗口未被清理 |

部署 SSH 只用于安装、启动 Ubuntu 测试及收取结果，业务协议和画面走 QUIC。任务专用 SSH master 最后已关闭，关闭后 Mac 仍能通过 QUIC 读取 Ubuntu 节点并刷新应用列表，最终实际界面显示已连接及 48 个已安装应用，测试入口已移除。没有重新引入产品 SSH 登录或返回通道，没有修改 EasyTier 设置。

旧安装备份分别位于工作区 `dist/upgrade-backups/macos-20261022/LAN RemoteApp.app` 和 Ubuntu `~/.local/state/lan-remoteapp/upgrade-backups/20261022-quic-only/`；备份不包含旧密码配置。当前 UI 版本号仍为开发预览 0.1.0，具体代码与文件身份以[部署记录和 SHA-256](validation-artifacts/2026-10-22-quic-upgrade.json)为准，[真实双向回归日志](validation-artifacts/2026-10-22-quic-upgrade-logs.json.gz)另存。本轮未重新测性能、三台以上实机、物理断网、休眠、签名公证、CI 或生产验收。

## 尚未覆盖

- 当前覆盖 xterm、独立 GTK 编辑器及原桌面 Firefox 的基础操作；现有 ChatGPT 的新版联调尚未完成。未完整验证复杂 GTK/Qt 应用、菜单树、透明窗口、GPU 程序及 Wayland 原生程序。
- 输入法基础预编辑/提交和主动双向文本、图片、文件剪贴板已实现并有独立测试；复杂中文输入法、精确候选位置、现有 ChatGPT 往返和附件界面、其他图像编辑器与文件管理器仍未完整验收。后台剪贴板同步不在当前范围。GTK 2× 重绘已验证；跨屏幕 DPI 热切换、Qt/传统 X11 应用、双屏、休眠与网络切换、生产性能和长时间稳定性尚未覆盖。
- 未做发布签名、公证、外部安全审计、CI 或生产验收。Ubuntu 已安装持久用户服务，尚未验证重启系统后的自动启动。Mac 发布辅助组件使用本机临时签名，不能据此宣称已完成发布签名。
- 会话与幂等表在内存；停止 Agent 会结束其子进程，不能将断线恢复通过解释为 Agent/系统重启后仍能恢复。

当前技术为配对 QUIC 可靠流、兼容 JSON Lines、H.264（PNG 回退）、XDamage / MIT-SHM 窗口采集（可回退 GetImage 轮询）、原生 GStreamer C API / OpenH264 软件编码与 Swift VideoToolbox/AppKit/CoreGraphics。QUIC 网络视频使用二进制负载，控制水位协调跨流顺序。Protobuf schema 为后续草案；WebRTC、QUIC DATAGRAM、Metal、精确远端文本上下文与文件拖放未实现。视频编码为有损 4:2:0；长时间稳定性、弱网丢包/拥塞和其他设备的性能尚未验收。


## 关联窗口与跨进程预览（2026-10-22）

本轮由飞书图片预览问题触发。Mac 的主窗口属于 PID 709，图片窗口属于其子进程 `Lark Helper` PID 65990；辅助进程是 accessory 应用，`NSRunningApplication.launchDate` 为空。Ubuntu 的主窗口属于 PID 44108，图片窗口属于其子进程 PID 1854186，且没有 `WM_TRANSIENT_FOR`、`WM_CLIENT_LEADER` 或 `WM_CLASS`。这说明只检查同一 PID、主应用启动时间或显式 transient 关系都会漏掉该场景。新版 Mac 扫描实际读到这两个窗口的有效内核身份。

上述飞书检查只读取窗口与进程元数据。用户手动打开图片，自动点击未作为成功证据；没有向聊天输入框写入或发送消息。后续交互和双机回归使用专用测试应用，不能将夹具通过写成全部飞书图片操作已验收。

实现分步提交：`fa21c65` 为 X11 跨进程跟随和 Mac 子窗口关闭，`06922fa` 为 GTK 多窗口接收，`0adc4f5` 为父窗口消失后的关联后代清理，`b9a3297` 为窗口级剪贴板失败处理，`2ba50db` 为多会话歧义拒绝，`dd4349a` 为 Mac 辅助窗口采集、输入和双机夹具。产品仍是一个程序双向使用，业务传输仍是 QUIC；SSH 只用于本次开发部署、执行测试与清理。

| 验证 | 实际结果 |
| --- | --- |
| Ubuntu → Mac，真实双机 QUIC | 实际 GTK 主窗口、显式对话框和无 transient 的独立子进程预览，经 H.264 显示为 AppKit 窗口；预览中文/emoji、复制、两次重连保留原生窗口、关闭预览后继续操作主窗口均通过 |
| Mac → Ubuntu，真实双机 QUIC | 真实 accessory 子进程经 SCK/VideoToolbox → QUIC → GTK/GStreamer；预览中文/emoji、原生复制、两次重连、独立关闭和断开保留源应用均通过 |
| Mac 本机生产发布代码 | 两个同 PID 独立主窗口隔离；辅助子进程单独采集、输入、复制；其他会话输入被拒绝；目录归属、重连与预览关闭均通过 |
| 三个真实 QUIC 节点 / GTK | 两个配对设备使用相同会话号和窗口号时，原生窗口、输入、解码器与关闭互不混用；迟到帧不复活窗口；窗口级复制失败只取消对应窗口；无窗口号的旧端失败也不会无限等待 |
| X11 隔离桌面 | 实际子进程创建窗口；伪造 PID/transient 不能越权；相同 PID 的其他主窗口不被接入；重连、中文、复制粘贴、独立关闭与原进程存活通过 |
| 内核身份和歧义规则 | PID/启动时间变化拒绝、窗口标识不重绑定、父窗口消失后的后代退役、同一应用多个会话近期输入时不推测弹窗归属，均通过 |
| 自动门禁 | Mac `scripts/check.sh` 完整通过（Rust 44 项通过、4 项按条件忽略；前端 22 项及构建；Python、Swift/AppKit 回归）；最终归属规则加固后另重跑对应 Rust/Swift 边界、Clippy、真实双机测试和 release 构建。Ubuntu Rust 47 项通过、4 项按条件忽略；最终根 workspace Clippy `-D warnings` 与格式检查通过 |
| 打包与包内检查 | Mac Tauri release 和 Linux 安装包构建通过；包内 Node/WindowHost 二进制与构建产物一致，Linux 接收源码与最终版本一致；交付包内 release WindowHost 和网络组件实际连接 Ubuntu 夹具通过 |

首次反向双机复制失败，原因是辅助窗口输入时只激活了主应用，未激活辅助进程的 Edit 菜单/响应对象。修复后本机和双机复制均通过；错误回包补充 `window_id`，接收端也兼容旧端没有窗口编号的失败。测试夹具同时修复了子进程退出清理、路径别名、事件监听保留以及隔离显示服务就绪检查，未把夹具故障归因于 QUIC。

可复现入口：`app_discovery_smoke.py`、`linux_receiver_smoke.py`、`mac_publisher_boundaries.py`；真实 Mac 发布代码使用 `mac_related_windows_smoke.py --directory /tmp/lra-related-mac --hold`。独立 Ubuntu 主机夹具为 `tests/fixtures/linux_related_host.py --directory /tmp/lra-related-linux`。在各自隔离状态目录启动网络组件并配对后，运行 `mac_related_receiver_live.py` 与 `mac_related_to_linux_live.py`；命令参数由各脚本的 `--help` 给出。这两个双机接收测试自身不调用 SSH。

开发产物为 `dist/lan-remoteapp-related-macos/LAN RemoteApp.app` 与 `dist/lan-remoteapp-linux-related.tar.gz`，**尚未覆盖当前运行的安装**。Node 的 ad-hoc bundle 校验通过；WindowHost 按既有流程提供未签名的应用模板，完整 bundle 签名检查不通过，Mach-O 校验和包内实际启动通过。不是正式签名、公证或生产发行产物。

边界：无法证明亲缘关系的外部应用/系统服务窗口、已经显示但无法归属的独立窗口，以及同一应用多个会话同时输入造成的歧义，不会自动接纳。没有完成三个物理设备并发、两个方向同时争用键盘/剪贴板、长时间压力、休眠唤醒、原生 Wayland 或所有应用兼容性验收；本轮未做性能对比，也未执行 CI。结构化条件、产物 SHA-256 与检查输出见 [JSON 记录](validation-artifacts/2026-10-22-related-windows.json)和[压缩日志](validation-artifacts/2026-10-22-related-windows-logs.json.gz)。

验收结束后已停止专用测试应用、发布器、接收器与网络组件，移除两端临时配对证书/状态及编译缓存，并关闭本次开发 SSH master。原 Ubuntu Agent 仍为 active；原 Mac 发布器可访问，共享、录屏与辅助功能状态正常。

## HexHub 启动停在 Logo（2026-10-22）

实机复现为 Ubuntu 已创建进程和独立会话，但前 15 秒没有窗口事件；随后只有 600×330 的启动 Logo，1400×815 主窗口保持 `IsUnMapped`。不是 QUIC 断链：同一连接可以接收 Logo 的 PNG。捕获启动日志后发现独立 D-Bus 的 `org.freedesktop.secrets` 激活超时，约 25 秒后出现 `OSCrypt tried Libsecret but couldn't initialise`。GPU 初始化也有警告，但在成功组仍然出现，不能据此认定 GPU 是阻塞原因。

同一 Ubuntu、HexHub 安装、用户资料、Xvfb 显示、工作目录、缩放和 `NoNewPrivileges=1` 下，仅改变应用的会话总线连接，依次观察：

| 会话总线条件 | 实际观察 |
| --- | --- |
| 原有独立 `dbus-run-session` | 约一分钟仍未显示主窗口；测试结束时停止自有进程 |
| 直接连接当前用户桌面总线（仅诊断） | 主窗口正常映射，10 秒检查时已显示 |
| 每会话过滤代理，仅允许 Secret Service | 首次约 1 秒采样时主窗口已显示；连续观察 31 秒保持正常 |

以上为定位故障的单次对照，非启动性能基准。最终实现采用第三组，默认应用继续使用独立总线。没有修改 HexHub 程序、用户资料、密码存储策略或系统密钥配置，没有添加禁用沙箱的参数。

验证通过：根 Rust workspace 测试与 Clippy `-D warnings`；Mac WindowHost 编译和自动重连回归；三节点 QUIC 路由隔离回归；Ubuntu `session_bus_smoke.py`（仅伪密钥服务）、`app_discovery_smoke.py`、`stale_launch_smoke.py`。初版总线测试夹具未带入 Openbox 主题目录，导致无窗口；补齐测试资源后通过，产品代码未因此调整。

已备份并更新 Ubuntu 的实际 `remoteapp-agent.service`，安装二进制 SHA-256 为 `9aa7e1e7ed602f669a8036d2261ebe7018d4fe15ed8d4f8a4047ecbb6cdb5e80`。从当前运行的 Mac 启动器点击 HexHub 后，实际 AppKit 窗口显示完整主界面及应用自己的更新提示，副标题为 **2940×1604、界面 200%、H.264**。本轮未操作 HexHub 中的账号、数据库连接或更新按钮；窗口保留供用户使用。Mac 接收端使用当前已有版本，本次新增的错误提示源码已编译但未重新打包安装。没有进行完整双向 E2E 或长期压力测试。

## 原窗口优先、像素与 GNOME Wayland（2026-10-22）

根因和逐层尺寸证据见[窗口像素与缩放](window-resolution.md)。原飞书是原桌面接管：1200×700 请求能到达真实窗口与 PNG，2940×1660 请求在窗口管理阶段就停在 1920×968；旧 Mac 自动模式继续铺满，放大并改变比例。原“100%”来自固定元数据，不是应用 DPI 测量。Firefox 是原生 Wayland，旧 X11 发现路径看不到原窗口，而新启动转交给已有实例后退出又被误报为成功。

本轮提交：`39cdb9a` 已有应用优先与失败提示；`3017564` 等比绘制、实际像素和缩放状态；`29f5946` 重连标题回归；`8badaf0` 仅新飞书实例的渲染器缩放参数；`0cdeff7` 隔离夹具退出顺序；`44c459c` GNOME 原窗口、PipeWire 及合成器尺寸控制；`6196eec` 保留活动服务的延期升级。

验证结果：

- Mac `scripts/check.sh` 完整通过：Rust 49 通过、4 忽略，前端 24 通过，Vue 类型/生产构建，Swift 发布器/宿主、Retina 与实时 resize、旧几何输入隔离、剪贴板顺序、自动重连、两个实际 QUIC 节点的 H.264 解码和独立二进制流回归通过。Clippy 全 workspace / all-targets `-D warnings` 通过。
- Ubuntu workspace 52 通过、4 忽略；Agent 显式包含 Linux 编码测试 15/15 通过。应用发现、原窗口输入/Unicode/复制/剪切/重连回归通过。Wayland 夹具验证原 PID/内容、PNG 1200×700/800×500、H.264 960×540、尺寸限制、租约/范围拒绝、未授权输入拒绝、原生剪切在删除前拒绝、重连复用。Xwayland 采集实际 2940×1660、2800×1500，并核对离屏客户点映射。上述 GNOME 测试在私有合成器中运行，没有接管真实 Firefox。
- 真实 Mac GUI 使用原飞书会话测试窗口模式、全屏、放大及断开：远端旧服务仍返回 1920×968，新宿主均等比显示且报告实际/目标。新独立 XTerm 目标 2940×1604，收到 2938×1603，符合字符网格取整；最后正常退出自建 XTerm。原 ChatGPT、ToDesk、HexHub、飞书和计算器五个会话仍保留，无测试控制端。
- Linux 安装生命周期测试通过：活动服务时普通安装在写入前拒绝；延期安装不 stop/start/restart，运行中旧二进制 inode 保留。Mac 和 Linux 开发包均构建成功，未做签名、公证或 CI。

过程中的失败也保留：一次临时目录清理与子进程写入竞争已修复后复跑通过；首次临时源码包漏复制图标导致打包失败，补齐图标后成功；直接从 SSH tty 启动私有 GNOME 出现窗口/握手超时，从用户 manager 启动复跑 4.657 秒通过，并为脚本补充运行上下文提示。这些不能当成产品 QUIC 性能或真实 Firefox 验收结果。

部署时保留原工作：Ubuntu 文件已通过 `--defer-restart` 安装，备份为 `/home/chenrg/.local/state/lan-remoteapp/upgrades/wayland-20261022-203949`。Agent PID 2102443、飞书 PID 44108、Firefox PID 2264753 及各自启动时间均未变；新 Agent 的 SHA-256 为 `21df5f150e94e5715234e3c0f01c02808adb60f80a2a618995907823d492db74`。扩展已安装并加入 enabled-extensions，当前 GNOME `GetExtensionInfo` 仍为空，需要下次图形登录才识别。没有自动注销、重启旧服务、退出飞书或修改全局缩放。

剩余实机验收：激活新扩展与 Agent 后的真实 Firefox 原窗口/账号/标签页、原飞书大像素重绘与鼠标坐标、新飞书 2× UI、多显示器和分数缩放；原生 Wayland 图片/文件剪贴板与安全剪切尚未实现。Mac 本机共享保持原来的关闭状态，没有将构建通过写成双向实机验收。所有条件、产物校验和及日志见 [JSON](validation-artifacts/2026-10-22-window-resolution.json)和[压缩日志](validation-artifacts/2026-10-22-window-resolution-logs.json.gz)。


## 2026-10-22 清晰度与 Snap 启动回归

- Mac `bash scripts/check.sh` 全部通过，包含 24 项前端测试、AppKit 原始像素尺寸/Retina 输入映射、重连与双节点 QUIC；`cargo clippy --workspace --all-targets -- -D warnings` 通过。受限进程不能使用系统剪贴板/VideoToolbox 时的首次执行失败，已在正常系统服务访问条件下重新通过；不算作应用自身通过。
- Ubuntu 真实 GStreamer/OpenH264 输出测试验证：静止刷新 PNG 与编码前原像素生成的 PNG 字节完全一致；下一帧继续原 H.264 流；几何变化取消待发旧刷新；未协商的 H.264-only 接收端不接收 PNG。
- 两个隔离 QUIC 节点的 `video_stream_smoke.py` 通过，导出 19 帧。Mac `video_decoder_smoke.py` 成功解码 18 张 H.264 硬件帧与 1 张 PNG，验证无损刷新后的 P 帧连续性、奇数尺寸和重连。该测试不等于真实 Firefox/飞书的视觉验收。
- `snap_service_launch_smoke.py` 在 Ubuntu 真实 Snap 下通过：旧 `NoNewPrivileges=yes` 策略稳定触发 snap-confine capability 拒绝；相同 `--version` 命令在修正后的用户服务策略下成功。全程不创建浏览器配置。安装生命周期隔离回归仍通过。

上述新版本现已部署。经用户保存工作、授权重新登录并恢复已选 GNOME 扩展，PaneFerry 扩展处于启用状态；系统仍为 GNOME 50.1 / Wayland。新独立飞书真实客户区为 2940×1604，带 2× 渲染参数，用户确认“清晰很多了，可以接受”。由于电脑工具未获准读取飞书画面，该项视觉结果以用户反馈为依据。

真实原生 Firefox 的同一 PID/窗口接管、1200×700 接收、PNG 无损刷新、指针、整段粘贴和回车导航已验证。自动化逐字输入仍有冒号缺失的未定位现象，不能将上述检查写成完整输入验收。当前原生 Wayland 窗口流仍受完整采集范围限制，未达到大 Mac 窗口的 Retina 像素目标；没有改用独立 Firefox 配置、切换桌面或全局缩放。完整边界、进程身份与二进制校验和见[激活后实机记录](window-resolution.md#激活后的实机结果2026-10-22)和[结构化记录](validation-artifacts/2026-10-22-window-activation.json)。此前“尚待激活”的段落是历史检查点，不是当前部署状态。

## 2026-10-22 后续：大窗口 Retina 与连续输入

提交 `afcaa7e` 使用每个租约独立的临时高 DPI 输出，让原生 Wayland 应用重新渲染，再创建窗口采集流；提交 `7bd8439` 把文字提交与主动粘贴分离，并同步 EIS、Clutter stage 和 text-input。两者按平台能力处理，不新增应用名称判断或 Firefox 配置。

| 检查 | 结果与范围 |
| --- | --- |
| 原生 Wayland 实际像素 | 私有 GNOME 50.1 中 PNG 为 1200×700、2940×1604、2940×1846、800×500；应用报告 2× 密度，另一个应用仍为 1×；重连后 H.264 960×540 正常 |
| 连续文字和顺序 | 三次完整独立回归，每次三轮快速全选、连续英文/标点/中文/emoji、随后立即回车；文本逐字一致，输入不修改剪贴板。无 text-input 焦点时明确拒绝且不修改内容，主动整段粘贴仍正常 |
| 旧问题对照 | 模拟应用延后 150ms 读取剪贴板，旧路径三次均把 `about:newtab-0123456789` 变成 `ut:newtab-0123456789999`。中间版本只等 GLib idle 仍出现“全选晚于文字”，补齐 EIS 与 stage 完成确认后通过三次复测 |
| 多设备与窗口隔离 | 两个并发租约占用不同输出，错误 owner/PID/窗口拒绝；释放恢复原几何/密度，空闲撤回输出，重连保留原进程及内容；物理显示器原布局不变 |
| X11 回归 | 原窗口中文、复制/粘贴/剪切、顺序、重连及进程保留通过；Xwayland 的 2940×1660、2800×1500 原始 PNG 与坐标换算通过 |
| 门禁 | Mac `scripts/check.sh` 通过：Rust 49 通过/5 忽略、前端 24 通过、Swift/AppKit、重连和两个隔离 QUIC 节点回归；Mac Clippy `-D warnings` 通过。Ubuntu workspace 52 通过/5 忽略；最终授权检查补丁后 Agent 14 通过/2 忽略、Python 授权 4 通过。Ubuntu 未安装 cargo-clippy，该项未执行 |
| 构建 | Ubuntu 发布/接收统一安装包、Mac WindowHost 和启动器 release 成功；新 Mac 完整包另存并通过 ad-hoc `codesign --verify --deep --strict`，未公证。Linux 包内新增 JS 和二进制与构建源/产物一致 |

三次完整隔离 GNOME 回归耗时 21.333、17.014、18.358 秒；整个测试进程组峰值内存约 2.7–2.8 GiB，含独立 Shell、应用与多个高 DPI 输出。这不是单个 Firefox 的资源或输入延迟基准。测试输入通过自有 Mutter 会话和生产 EIS 类驱动，未冒充真实用户 Portal 授权；Agent 未获授权的键、文本和粘贴均被拒绝。

**此次新改进尚未激活到真实桌面。** GNOME 50 会缓存已导入的扩展模块，禁用/启用不能加载新代码；当前 Agent 又拥有正在使用的独立飞书进程。为遵守保留会话的要求，未注销 GNOME、未重启 Agent、未关闭 Firefox/飞书，也未覆盖正在运行的 Mac 应用包。Firefox PID 2973421/启动时间 29932989、飞书 PID 3056368/30061642、Agent PID 3032522/30026538、Shell PID 2904453/29789695 均保持。没有读取或更换 Firefox 账号资料、标签页配置。

待切换的 Mac 包为 `dist/retina-input-20261022/窗渡 PaneFerry.app`；Ubuntu 包在 `/home/chenrg/.local/state/lan-remoteapp/upgrades/retina-input-20261022/paneferry-linux`。需要用户安排保存工作后的激活，再核验真实 Firefox 的大窗口、连续输入、原账号及标签页；不能把 GTK 夹具通过写成 Firefox 已验收。Mac 本机反向发布、多物理设备并发、混合/分数缩放、长期压力和 CI 本轮未验收。结构化证据、日志摘要与包校验和见[记录](validation-artifacts/2026-10-22-retina-input.json)；完整本地日志保存在上述 Mac 包同级的 `validation/`。

用户随后明确选择“保留当前会话，稍后再切换”。激活状态记为用户暂缓，新版安装包保留；没有安排自动切换，待用户再次要求后继续激活与实机验收。

### 2026-10-22：保留会话时补齐 VPN 开机启动

用户反馈重启后 VPN 难以恢复。检查发现 EasyTier 当前连接由 `easytier-recovery.service` 临时单元提供，单元位于 `/run/systemd/transient/`，网络配置位于 `/run/easytier-recovery/network.toml`，均不会跨重启保留。RustDesk 系统服务已启用开机启动；有线连接 `netplan-enp1s0` 已自动连接且不限制登录用户。

已在 Ubuntu 写入同名持久单元 `/etc/systemd/system/easytier-recovery.service` 和开机依赖链接；配置原样复制到 `/etc/easytier/recovery/network.toml`，仅 root 可读写（0600），目录 0700，网络密钥未输出或写入仓库。持久单元等待 `network-online.target`，使用原二进制和网络配置，退出后每 5 秒重试。没有启动第二份相同网络、没有重启现有服务或注销桌面。`systemd-analyze verify` 通过，排除临时目录的开机单元检查返回 `enabled`，当前 EasyTier PID 2900526 保持且为 active。运行中的临时单元仍承载当前连接，持久单元供下次开机使用。

此次仅验证持久配置、开机依赖和当前连接保留；尚未重启验收，也未验证 RustDesk 在登录界面的实际控制能力。此项不改变 PaneFerry 新版暂缓激活的决定。


## 2026-10-23：按用户要求回滚，Ubuntu 运行会话暂缓切换

用户要求恢复到“那就按统一方案改，覆盖 Wayland 和 Xwayland…”这条指令时的状态，随后明确选择“还有任务运行，先保留 Ubuntu 会话”。因此本轮仅完成代码与 Mac 运行程序回滚，**整套回滚尚未完成**。

代码通过提交 2fd332f 撤销 9b79da8..ca3ded1，提交后的树与统一 DPI 改造前 9b79da8 完全相同；这条记录另行提交，不属于原代码。原改动和证据仍在 Git 历史与 codex/pre-unified-rollback-20261023 备份分支，没有强制重置或覆盖用户未提交工作。VPN 持久化记录位于目标之前，保留。

Mac 已从 dist/activation-backup-20261023-100931/窗渡 PaneFerry.app 恢复到原活动路径并启动，使用当时实际运行的包，没有激活当时另行暂存的 Retina/input 包。旧备份缺少完整外层资源封印，恢复后续签外层 ad-hoc 签名；签名会改变 Mach-O 签名/链接元数据，不能直接把原始文件 SHA 当作签后相等。已核对签名前四个组件与旧包一致、签后启动器 15 个有文件数据的 Mach-O 代码/数据 section 全部相同、网络与 WindowHost/Node 全文件 SHA 不变，外层深度/严格验签通过。没有为了通过验证编写新产品修复。回滚前的新运行包也已备份。

Ubuntu 没有替换已安装文件、重启服务或注销；Agent 仍是新版 da9c08…，窗口桥仍报告版本4。Agent1007878、网络995120、GNOME998690、飞书1034686、ChatGPT1002994、EasyTier2900526、RustDesk52713 的 PID 与内核启动时间未变，账号目录未修改。Mac Node15189 保留。整机未重启。用户保存任务并安排一次桌面重新登录后，仍需恢复 Ubuntu 旧服务/窗口桥，不能把当前混合运行状态称为整套回滚完成。

回滚源码的 scripts/check.sh 通过，170.96 秒：Rust49通过/5忽略、前端24通过，以及 Swift/AppKit 和隔离真实 QUIC/视频/二进制传输。恢复的 Mac 网络组件与未切换的 Ubuntu 真实 QUIC 握手、52项应用、窗口、会话列表可读，配对身份保留。本轮未操作真实应用做画面或长期稳定性验收，未据此宣布闪退已解决。校验、日志、备份与未完成范围见[回滚记录](validation-artifacts/2026-10-23-rollback.json)。

## 2026-10-23：按 Mac 逻辑尺寸适配，保留正在运行的 Ubuntu 会话

用户改为采用逻辑窗口大小匹配、等比显示与对应鼠标坐标的方案，并要求保留账号、会话和当前任务，不重启电脑。提交 `249a02e` 仅修改 Mac 接收端和启动器的显示策略；没有恢复此前撤回的 DPI 切换请求，也没有新增应用名称判断。

原自动模式使用 Mac 内容区 points × Mac backingScaleFactor 请求远端像素；例如 1470×802 点的内容区在 Retina 上会请求 2940×1604，即使源应用仍为 1×。本轮改为 points × 源窗口已报告的 scale_factor：源 1× 请求 1470×802，源已为 2× 时请求 2940×1604，两者都对应约 1470×802 的逻辑布局。只发送原有 window.configure；不发送 window.viewport 或改变远端缩放。新启动默认请求 100%，已有会话仍沿用原缩放。保留实际画面等比显示和鼠标按图像矩形换算的实现，并扩大回归。标题区分本机逻辑点、实际接收像素和未匹配的目标，修正了源 1× 在 Retina 上被误报为达到像素预算的问题。

| 验证 | 结果与边界 |
| --- | --- |
| Mac AppKit | 源 1×/2× 下同为 1470×802 逻辑大小；拖动、旧尺寸回包、固定/自动切换、留边、中心/边缘坐标、越界拖动释放、旧画面与新几何不一致时拒绝新点击通过 |
| 当前 Ubuntu 版本兼容 | 在私有 GNOME 中使用已安装 Agent 和复制的 v4 窗口桥，Wayland/Xwayland 都收到 1470×802、900×600、1470×900、700×500 的真实 PNG；源缩放保持 1×，重连复用同一进程、内容和会话 |
| 旧 Agent 兼容 | 使用激活前 Agent 备份及当前源码 v2 桥的隔离组合；Wayland 源 2× 接收到上述逻辑尺寸的两倍像素，Xwayland 源 1× 收到同尺寸像素；逻辑布局与会话内容保持。该组合不等于旧生产环境完整复现 |
| 项目门禁 | scripts/check.sh 通过：Rust 49 通过/5 忽略、前端 24 通过、Swift/AppKit、重连、H.264、两个隔离 QUIC 节点及二进制剪贴板；独立启动器 5 个 Rust 测试、格式检查和 Clippy all-targets -D warnings 通过 |
| 打包程序 | Release 窗口程序和按启动器方式复制出的命名入口均通过隔离 QUIC/H.264 启动检查，保持运行并请求 1470×802。此为测试窗口，不是 Firefox/飞书闪退的实机验收 |
| 部署 | Mac 启动器和 WindowHost 已替换并启动；完整包 ad-hoc 深度/严格验签通过。原网络进程 37982、Node15189 保持，文件 SHA 不变。Ubuntu 没有替换文件、重启服务或注销 |

Ubuntu Agent1007878、网络995120、GNOME998690、飞书1034686、ChatGPT1002994、EasyTier2900526、RustDesk52713 的 PID 和内核启动时间在更新前后相同。真实 QUIC 连接、52 项应用、运行窗口和 4 个原会话列表读取正常。原账号和配置目录未修改，未输入真实应用或关闭其窗口。旧 Mac 包备份在 dist/logical-size-20261023/previous/，新包与校验在同级目录。

此次没有操作真实任务窗口验证视觉效果、用户桌面的实际鼠标注入、系统全屏切换、多物理屏幕或长期稳定性；这些不能由夹具通过替代。1× 源画面显示在 Retina 上仍会放大，不能宣称获得原生 Retina 细节。协议仍沿用已有 1×/2× 源倍率，没有新增分数缩放协商。之前的 Ubuntu 整套回滚继续按用户要求暂缓，本轮 Mac 显示更新不依赖它。完整校验和日志见[结构化记录](validation-artifacts/2026-10-23-logical-window-size.json)和[压缩日志](validation-artifacts/2026-10-23-logical-window-size-logs.json.gz)。

## 2026-10-23：GNOME 桌面崩溃，停止窗口桥并修复失效窗口清理

用户随后报告 Ubuntu 桌面重启。11:37:43 的 GNOME 日志明确记录 SIGSEGV，JS 堆栈指向已部署 `virtualOutput.js:126` 的 `root.move_to_monitor()` 和第 107 行的异步清理。原生 core 的 GJS/FFI 调用进入 libmutter-18，故障指令在读取 monitor 字段后访问空指针偏移 0x18。窗口已经 unmanaged，JS 包装对象仍存在，原来的 try/catch 无法拦截原生段错误。[Mutter 50 源码](https://github.com/GNOME/mutter/blob/50.0/src/core/window.c)也明确区分了可返回 -1 的 get_monitor 与直接解引用 monitor 的 move_to_monitor_internal。

这属于 PaneFerry 窗口桥引起的桌面崩溃，不能将责任归于网络或用户操作。整机启动时间仍为 10 月 19 日 10:26，Agent、网络、EasyTier 和 RustDesk 保持原进程；旧 GNOME998690、飞书1034686、ChatGPT1002994 已退出，新 GNOME 为1309605。上一节所述会话保留仅是 11:27 更新核验当时的状态，**不代表之后持续稳定，更不代表此次任务已恢复**。本轮没有修改应用账号、配置或标签页文件；未确认中断任务能否恢复。

先确认新桌面只有物理 DP-2、没有遗留虚拟显示器，再单独禁用 paneferry@local，已核验 INACTIVE。其余扩展、桌面、Agent、网络和用户应用均未由本轮重启。依赖此窗口桥的远程接管暂时不可用。

提交 `51d43d9` 在窗口开始 unmanaging 时立即撤销可操作的 root 引用，并在原生操作前检查 monitor。这个观察器独立于 lease，覆盖空闲释放已移除 lease.output、异步清理仍未完成的情况。创建过程跨异步边界再次检查窗口有效性，关闭操作保持幂等，串行清理并等待 connector 消失；窗口已关闭时仅回收虚拟显示器，不再移动失效窗口。没有修改应用缩放、账号、输入协议或做应用名称适配。

| 验证 | 实际结果 |
| --- | --- |
| 旧版隔离复现 | 测试专属 GNOME1351980 在第一次关闭夹具窗口后崩溃，内核记录与生产相同的 libmutter 偏移 0x120aea / 地址 0x18；测试收到 DisplayConfig 超时，退出1。系统产生 Apport 通知，正式桌面1309605保持运行 |
| 修复版生命周期 | 已部署 v4 的复制件只替换 virtualOutput.js：3次正常关闭、3次创建期间进程退出，以及活窗口的断开、空闲回收、禁用扩展均通过；旁边的夹具进程和内容保持。当前源码 v2 桥搭配旧 Agent 的隔离组合也通过 |
| 逻辑尺寸回归 | 修复后的 v4 复制件，Wayland/Xwayland 均收到1470×802、900×600、1470×900、700×500实际像素，源倍率保持1×，重连复用同一进程、内容与会话 |
| 本地门禁 | 新增11项生命周期竞态检查；完整 scripts/check.sh 退出0，Rust49通过/5忽略、前端24通过，Swift/AppKit、重连和隔离QUIC/H.264通过 |

远端测试使用私有 D-Bus、HOME、配置和无头 GNOME，限制内存4G、CPU200%、运行时间120秒；没有操作真实飞书或 ChatGPT。Linux 复制件测试的内存峰值2.2G。旧版只复现一次，不在正式桌面重现。

修复 **尚未激活到正式桌面**。已保存到 `/home/chenrg/.local/state/lan-remoteapp/upgrades/window-lifecycle-20261023-1154/`，同时保留原扩展副本；manifest确认只有virtualOutput.js变化。正式安装文件仍未覆盖，扩展保持停用。GNOME缓存已导入模块，同一桌面里禁用再启用不能证明新源码生效；遵守保留当前任务的要求，本轮没有注销或重启电脑。原先的Ubuntu整套回滚仍未完成，不能借本补丁悄悄切换Agent。

清晰度改进暂缓；此次没有声称解决1×源在Retina上的细节不足，也没有把隔离回归当成真实应用或长期稳定性验收。事故证据、校验和、边界见[结构化记录](validation-artifacts/2026-10-23-window-lifecycle-crash.json)，测试输出见[压缩日志](validation-artifacts/2026-10-23-window-lifecycle-logs.json.gz)。原始core和应用私密内容没有复制进仓库。


## 2026-10-23：Mac 可选 PNG 清晰对照，H.264 保持默认

用户要求只改 Mac、增加统一 PNG 模式并对比清晰度和流畅度，保持窗口大小、比例、鼠标映射，禁止操作 Ubuntu 桌面或启用窗口桥。`d100138` 增加显示菜单和原会话重连切换；`8b7a418` 增加完全离线的接收端对比工具。用户随后质疑 PNG 的必要性，`c0dd248` 恢复 H.264 默认，PNG 仅作可选对照。产品改动仅限 Mac 的 main.swift 和 DisplayQuality.swift，没有应用名称分支，也没有修改 HostedWindow、SurfaceView、RenderGeometry 或 Linux 组件。

完整 scripts/check.sh 通过（Rust 49通过/5忽略、前端24通过、Swift/AppKit、两个隔离 QUIC 节点、视频和二进制剪贴板）；最终默认值修正后重新运行画质回归和 release 构建通过。PNG→H.264→PNG 保留同一 session、NSWindow、关联窗口、固定/自动模式及鼠标中心坐标；模拟断线后保持选择，无 H.264 能力时回退 PNG，剪贴板忙时禁用切换。真实本机 QUIC 测试也核对 H.264→PNG 的原会话接续和 PNG 原始像素，没有 app.launch。打包 PNG/H.264 入口保持运行并请求 1470×802。

Mac 已安装最终 H.264 默认版本。原 Node15189、网络37982、启动器49414 的 PID和启动时间保持；网络与 Node文件SHA不变。只用目录交换更新包，旧映射文件保留在 dist/png-quality-20261023/running-before/，没有原地重写正在执行的文件。签名深度/严格检查通过；启动器代码未重新编译，重新封印使签名元数据改变。原包、新包、校验和、安装记录在 dist/png-quality-20261023/。

本轮没有访问 Ubuntu、重启/注销任何桌面、启用窗口桥、修改账号配置或操作真实应用。此前窗口桥停用和 Ubuntu 回滚暂缓的状态保持，不能以本地回归替代真实飞书、GPT 或 Firefox验收。PNG只是避免视频压缩损失，不能增加源图细节。[对比表、复现方法与限制](png-quality-comparison.md)，[结构化记录](validation-artifacts/2026-10-23-png-quality.json)。


## 2026-10-23：900×700 逻辑窗口隔离试验

按用户给出的六步流程，提交 `87d72fd` 增加 900×700 定点回归；没有修改产品代码或启用用户 GNOME 窗口桥。Mac AppKit 测试在源 1×/2× 时分别发送 900×700/1800×1400 `window.configure`，新画面到达后中心鼠标坐标分别为 450×350/900×700；没有改变窗口比例或发送全局 DPI 请求。

Ubuntu 使用正在安装的 Agent 二进制和先前暂存、已修复失效窗口清理的 v4 桥**复制件**，在私有 D-Bus、GNOME、Xvfb 和 GTK 小窗口中运行，限制 4 GiB 内存、200% CPU、130 秒。明确请求 Wayland 1× 时，900×700 逻辑窗口收到真实 900×700 PNG；请求 Wayland 2× 时，GTK 报告 2×，相同逻辑窗口收到真实 1800×1400 PNG，GTK 子控件重新分配为 900 逻辑宽。Xwayland 两次均为 1×、900×700 PNG，子控件也按 900 逻辑宽重排。两端夹具的进程、内容和会话在重连后保留，私有虚拟输出清理完成。

GNOME 正式桌面 PID1309605、Agent1007878、ChatGPT1314034 的启动时间保持；`paneferry@local` 前后均为 INACTIVE，只剩正式一个 GNOME Shell。没有注销、重启、切换真实应用或账号。本结果表明逻辑尺寸重排已经成立，Wayland 明确请求 2× 时能产生真实 2× 源像素；它不能证明当前默认 1× 连接或 Xwayland 已有 Retina 清晰度。H.264 实机清晰度、用户飞书/GPT/Firefox 和 Portal 输入仍未验收。[结构化试验记录](validation-artifacts/2026-10-23-900x700-probe.json)。

## 2026-10-23：真实应用的独立 GNOME 重绘试验

在私有 D-Bus、HOME、配置和无头 GNOME 中，以 `gnome-text-editor --standalone` 打开临时文本；没有启动或读取用户现有应用。首次请求 900×700 逻辑尺寸时，未修补的桥虽然报告 1800×1400 PNG，画面右下仍为黑色，20 秒内未自行更新；重复同尺寸及等待 1 秒均无效。仅将逻辑尺寸暂调 1 像素再回到目标尺寸，真实应用即完整重绘。`8f66679` 将这一步限定在 Wayland 窗口首次使用某个私有输出时执行，按窗口跟踪，不改变 Xwayland 路径，也不触碰全局桌面缩放。

当前源码构建的 Agent 与桥在隔离会话中通过：900×700 点收到完整 1800×1400 像素，随后 850×650 点收到完整 1700×1300；同一编辑器 PID 保持，右下像素均为白色，输出释放后虚拟显示器清理完成。当前 Ubuntu 已安装的 v4 Agent 搭配同样修复的 v4 桥复制件也分别通过 1×/2×：900×700 点收到 900×700/1800×1400，后续缩放亦完整。[未修补首帧](validation-artifacts/2026-10-23-editor-2x-partial-before.png)、修补后 [1×](validation-artifacts/2026-10-23-editor-1x.png) 和 [2×](validation-artifacts/2026-10-23-editor-2x.png) 画面均保存在仓库；[结构化记录](validation-artifacts/2026-10-23-real-app-redraw.json) 包含运行条件与校验和。原有 Wayland/Xwayland 逻辑尺寸回归和完整 `scripts/check.sh` 通过。

Firefox 的 Snap 启动器要求会话 systemd，不能在该私有 D-Bus 中启动；Chrome 155 在该无头会话里于窗口创建前 SIGSEGV。这两项未进入 PaneFerry 采集，不能据此判断真实 Firefox、飞书或 ChatGPT 的画质。1×/2× 图片是原始 PNG 源像素，不是 Mac 端 H.264 显示验收；现有 Mac 启动仍默认请求 1×，现有 Xwayland 窗口仍是 1×。修复仅提交源码，未部署到正式 Ubuntu；正式 GNOME1309605、Agent1007878、ChatGPT1314034 保持，窗口桥仍为 INACTIVE，没有注销或重启。

## 2026-10-24：新独立 X11 会话的 DPI 同步

新增私有 Xvfb 的 96/192 物理 DPI 与 `Xft.dpi`，并调整 GTK/Qt 的通用启动环境；没有修改正式 Ubuntu 桌面的资源数据库或已有应用进程。Mac 现有 Retina 新启动请求会选择 2×，但优先接管的已有窗口仍使用其原密度。具体实现边界见[窗口分辨率说明](window-resolution.md#2026-10-24新独立-x11-会话的通用-dpi)。

| 验证 | 实际结果 |
| --- | --- |
| Ubuntu 隔离构建 | 仓库副本在 `/tmp` 下执行 `cargo build --locked -p paneferry-agent -j 2` 通过；没有替换正在运行的 Agent |
| Ubuntu 隔离 DPI | `hidpi_smoke.py` 通过：`xdpyinfo` 96/192 DPI，`xrdb` 的 `Xft.dpi` 为 96/192；同一 GTK 窗口逻辑尺寸 600×400，实际 X11 客户区 600×400/1200×800，Pango 逻辑字体高度均为 19；Xft 终端窗口 604×232/1144×460 像素 |
| Mac 全项目门禁 | `bash scripts/check.sh` 退出 0，Rust/启动器/Swift/AppKit/隔离 QUIC 与 H.264 回归通过；`git diff --check` 通过 |
| Clippy | `paneferry-backend-x11 --all-targets -D warnings` 通过。全工作区严格检查未通过：Agent 有 5 处此次改动前已存在的告警（dead_code 2、needless_lifetimes、if_same_then_else、assertions_on_constants）；仅对这四种既有 lint 放宽后，Agent `--all-targets -D warnings` 通过 |
| 正式服务保护 | 测试结束后 ChatGPT PID 14785 仍在；`remoteapp-agent.service` 与 `paneferry-network.service` 仍为 active。没有注销、重启、切换正式 Agent 或读取账号内容 |

上述验证证明新私有 X11 应用能收到真实 2× 源像素及稳定逻辑字号；未验证真实 Qt/Electron 应用、Mac 端 H.264 画质、当前桌面的 Wayland/Xwayland 窗口或长期交互。当前正式版本没有部署本改动，不能宣称用户正在看的 ChatGPT 已变清晰。
