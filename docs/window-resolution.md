# 窗口像素、界面缩放与 Wayland

本页区分已定位的问题、代码行为和实机验收。Firefox 的目标是连接原窗口、账号和标签页；飞书目标是窗口重新布局后采集原始像素，不把低分辨率截图拉伸为目标尺寸。

下列初始证据保留激活前的状态。用户保存工作并授权重新登录后，扩展、新 Agent 和清晰度版本已经启用；最新真实应用结果见文末“激活后的实机结果”。

## 2026-10-22 根因与证据

实机为 macOS Retina 控制端与 Ubuntu GNOME 50.1 / Xwayland 发布端，通过已配对 QUIC 通信。SSH 只用于开发构建、读取进程状态和部署。Firefox 为 Snap 154，原进程使用 `GDK_BACKEND=wayland`；飞书原进程来自 `/opt/bytedance/feishu/feishu`。

| 对象 | 观察 | 可得结论 |
| --- | --- | --- |
| 原飞书会话 | display 0、原进程 PID 44108、窗口 48234500；不是新 Xvfb 实例 | 此次是原桌面窗口接管，之后恢复同一会话 |
| 原飞书小尺寸请求 | 请求 1200×700；实际窗口元数据与 PNG 都达到 1200×700 | 配置请求和采集路径可正常传递尺寸 |
| 原飞书大尺寸请求 | 请求 2940×1660；实际窗口元数据和 PNG 都停在 1920×968 | 尺寸在窗口管理阶段就没有达到目标，早于编码；不能归因于 H.264 |
| 旧 Mac 自动绘制 | 自动模式把任何图像画满内容区 | 尺寸未匹配时出现放大、比例变化；修复为始终等比显示 |
| 旧“界面 100%” | 已有桌面窗口的 `scale_factor` 固定为 1 | 是协议值，不是飞书实际 DPI 测量 |
| Firefox 启动失败 | 原窗口是原生 Wayland；旧端只查 X11；新启动很快退出且日志显示 D-Bus 转交成功 | 原 Firefox 没退出；请求转交给已有窗口后，旧端既看不到该窗口，又误把进程启动当成成功 |
| 同版本隔离 GNOME 的 Xwayland | 1600×900 虚拟显示器上，用户尺寸请求得到实际 2940×1660 和 2800×1500 的 XComposite PNG | 合成器路径能绕过普通程序尺寸请求的工作区约束；不能据此宣称真实飞书已通过 |

飞书小/大尺寸探测后恢复 1920×968 并断开控制；没有退出原飞书、改变桌面缩放或读取聊天输入。首次定位没有认定显卡、编码器或屏幕物理尺寸是固定硬上限。

## 三个不同的量

1. **Mac 内容区**使用 points；乘 `backingScaleFactor` 得期望像素。例如 1470×830 points、2× 屏幕得到 2940×1660 像素。标题栏与外框不计入内容区。
2. **实际画面像素**来自远端真实窗口，再经过采集、编码、解码。标题使用成功解码图像的尺寸。H.264 的偶数补边在显示前裁掉，不冒充窗口尺寸。
3. **应用界面缩放**决定字体、按钮和布局大小。新实例的 1×/2× 参数只是偏好；已有应用保留其当前缩放。既不能把 `scale_factor=2` 当成实测 DPI，也不能仅靠增大采集画面让控件变大。

自动目标按比例受 4096×2160、800 万像素预算限制。实际图像尚未达到目标、远端拒绝请求或达到预算时，自动和手动模式都等比显示并留边；标题明确显示接收像素、目标和限制。旧图像与新窗口几何版本不一致时丢弃新点击，释放动作仍要完成，避免点错位置或卡住按键。

清晰度还受两个独立因素影响：低分辨率画面在 Retina 内容区放大，以及 H.264/I420 对文字和彩色边缘的有损压缩。当前窗口画面只传 H.264；接收端按所在显示器刷新率和远端能力自动协商采集上限，协议最高 90 fps。为保持每帧质量，编码码率预算随帧率提高。实际帧率受应用重绘、采集、编码和网络限制，静止画面不会重复发送。这不会制造更多源像素，也不会改变应用的界面缩放。

Mac 标题会显示实际画面的放大百分比；“显示 → 原始像素大小”按当前屏幕 backing scale 调整内容区，使一个源像素对应一个屏幕像素，并固定当前实际分辨率。该选项可用于辨认放大造成的模糊；1× 远端应用的控件在 Retina 上可能偏小。需要大窗口和合理控件大小时仍须真正提高远端渲染像素并配置应用缩放，不能以无损编码替代分辨率适配。

## 已有窗口与新独立实例

点击应用入口先查同一应用的已有会话，再查原桌面窗口，最后才新建实例。有多个候选要求选择，有控制端则报告占用。十秒内没有可见窗口返回待确认并保留请求 ID，提前退出明确失败，不把退出码 0 当成已经有画面。

已有飞书仍由其原工具包决定 UI 缩放。已检查安装包装器会转发参数，渲染库支持 Chromium 的 `force-device-scale-factor`；仅对准确匹配的飞书桌面入口的新独立实例添加该参数，保留用户显式参数。GTK/Qt 环境变量不是飞书缩放已生效的证据。激活前没有为测试退出原应用；用户授权重新登录后，新独立实例的清晰度与按钮大小已获得用户认可，具体证据和未验证范围见文末。此结果不能替代已有桌面窗口的缩放验收。

Firefox 接管只使用合成器识别的原 PID / 窗口，不创建或复制浏览器配置。无法获得原窗口时明确提示，不自动改用隔离资料。新 Wayland 路径和原 X11 路径共用 Agent、QUIC、会话控制和设备隔离。

## GNOME 50 采集与尺寸限制

`paneferry@local` 扩展提供窗口列表、目标租约和尺寸操作。租约限定本地 D-Bus owner、原窗口 ID/PID、明确 transient 链；列表只读取元数据。关闭连接或锁屏不结束原应用；其他设备和其他主窗口不能借此扩大控制范围。

- **原生 Wayland**：Mutter RecordWindow 提供 PipeWire 流。GNOME 50 的窗口流采用显示器大小的缓冲区，实际区域由 crop 元数据描述。GStreamer 直接提取原始 BGRx 区域并去除阴影，不先做 videoconvert 丢掉 crop，也不通过缩放制造目标像素。超出完整采集范围明确返回 `resolution_limited`。该路径暂不支持超过显示器采集范围的原生窗口。
- **Xwayland**：保留 XComposite 像素采集，用合成器 `move_resize_frame(true, …)` 处理明确的用户尺寸请求，区分客户区、装饰和合成器坐标。窗口可比原显示器更大；输入先校验实际客户区尺寸，仅移动被选窗口，使对应点处于可输入范围。该移动会体现在 Ubuntu 本机桌面。
- **输入和剪贴板**：键鼠仍须系统 RemoteDesktop Portal 授权。原生 Wayland 当前只有文本复制/粘贴，剪切在删除前拒绝，图片/文件剪贴板明确不支持。实际 Firefox 已验证指针、选中地址栏、整段文本粘贴及回车导航；连续输入和完整剪贴板往返仍有未验收项，见文末。

依据同版本源码：[Mutter 尺寸约束](https://raw.githubusercontent.com/GNOME/mutter/50.1/src/core/constraints.c)、[窗口流缓冲区](https://raw.githubusercontent.com/GNOME/mutter/50.1/src/backends/meta-screen-cast-window-stream.c)、[窗口采集及 crop](https://raw.githubusercontent.com/GNOME/mutter/50.1/src/backends/meta-screen-cast-window-stream-src.c)。平台行为通过下述隔离桌面实测交叉核对；其他 GNOME 版本和其他 Wayland 合成器不在当前支持范围。

## 验证范围

| 路径 | 已测内容 | 剩余边界 |
| --- | --- | --- |
| 真实飞书 → Mac | 使用新宿主连接原会话；窗口模式目标 2940×1604、全屏目标 2940×1846、放大目标 2900×1564；实际仍为 1920×968；均等比留边且标题诚实；退出全屏、关闭宿主后会话保留 | 原桌面扩展未启用，未验收真实飞书大像素重绘或 2× 控件大小 |
| 新独立 XTerm → Mac | 目标 2940×1604，收到 H.264 2938×1603；字符网格取整如实显示，没有假报匹配 | XTerm 不采用 GTK/Qt UI 缩放；不据此证明飞书缩放 |
| 私有 GNOME 原生 GTK 窗口 | 独立 HOME/总线/合成器，原 PID/内容保持；PNG 1200×700、800×500；H.264 960×540；重连复用；越界尺寸、错误 PID/租约/外部窗口、未授权输入拒绝 | 不等于真实 Firefox、真实 portal 输入、多显示器或分数缩放 |
| 私有 GNOME Xwayland 窗口 | 实际 PNG 2940×1660、2800×1500；离屏客户点 (2890,1600) 经移动后映射到 (1591,891)，与实际窗口原点核对 | 几何核对，不等于真实飞书点击验收 |
| AppKit 回归 | Retina points/pixels、实时 resize、旧图像/新几何隔离、比例留边、手动/自动切换、重连及 H.264 解码 | 程序化窗口测试不代替双机 GUI 全流程 |

可复现命令（Ubuntu 需已安装 GNOME 50、PipeWire、Python GI、GTK3、Xvfb、Xwayland、GStreamer）：

```sh
cargo test --workspace
cargo test -p paneferry-agent -- --include-ignored
cargo build -p paneferry-agent
python3 tests/e2e/app_discovery_smoke.py --agent target/debug/paneferry-agent
python3 tests/e2e/existing_window_smoke.py --agent target/debug/paneferry-agent --controller-text
python3 tests/e2e/wayland_smoke.py --agent target/debug/paneferry-agent --with-xwayland
```

`wayland_smoke.py` 创建自己的合成器、总线、应用、HOME 与显示，只借用当前用户的 PipeWire 服务；没有重启真实桌面或打开 Firefox。Mac 门禁为 `bash scripts/check.sh`；安装生命周期另测 `python3 tests/e2e/linux_install_safety.py`。

Wayland 夹具从已登录桌面的终端运行。若从 SSH 执行，通过当前用户的 manager 启动，例如 `systemd-run --user --collect --wait --pipe --working-directory="$PWD" python3 tests/e2e/wayland_smoke.py --agent target/debug/paneferry-agent --with-xwayland`。本次直接在 SSH 的 tty 会话启动私有 GNOME 时出现窗口/握手超时；通过用户 manager 复跑 4.657 秒通过。该差异属于隔离合成器的运行上下文，测试提前解释此限制，不因此更改产品权限或真实桌面。

首次安装的 GNOME 扩展在当前登录中可能未注册，需要下次登录才生效。不得为了激活而自动注销桌面；注销会结束原应用，重启 Agent 也会结束其自建独立会话。安装准备完成不等于真实 Firefox 或飞书大分辨率已经验收，必须在用户保存工作并完成激活后继续测试。

## 激活后的实机结果（2026-10-22）

用户保存工作并授权注销/重新登录后，仍为 GNOME 50.1 / Wayland，没有切换桌面或改变全局缩放。GNOME 此前因 Shell 失败关闭了用户扩展总开关；按用户明确选择，恢复已选扩展并启用 PaneFerry。`GetExtensionInfo` 返回 `state=1`、`enabled=true`、`error=''`。恢复扩展时 Shell PID 2904453 未改变。

| 对象 | 已确认结果 | 验收边界 |
| --- | --- | --- |
| 新独立飞书 | PID 3056368，Xvfb `:288`，实际参数 `--force-device-scale-factor=2`；Mac 调整窗口后，`xwininfo` 确认客户区 **2940×1604**、`IsViewable`；用户反馈“清晰很多了，可以接受” | 不是注销前的原桌面飞书。工具未获准读取飞书画面，Mac 解码像素及视觉效果未独立复核；用户反馈是本次视觉验收依据，不把 2× 参数当作精确 DPI 测量 |
| 原生 Wayland Firefox | 正常 Firefox 配置启动的 PID 2973421，未添加 profile/no-remote 参数；三个原生主窗口可被发现，选择窗口 32 后接管同一 PID；真实窗口和 Mac 标题均为 **1200×700**，视频停下后显示 PNG 无损刷新 | 注销前 PID 已随获准注销结束。没有创建或复制资料，也没有读取账号内容；未核验注销前每个标签页/账号的恢复 |
| Firefox 输入 | 指针定位地址栏、Ctrl+L、整段粘贴 `about:newtab`、回车打开本地新标签页成功；单独 Shift+分号能输入冒号 | 自动化 `typeText('about:newtab')` 时冒号缺失，整段粘贴成功；尚不能归因于网络、协议或应用丢字。连续逐字输入、中文输入及复制回传未判定通过 |
| Firefox 大窗口 | 当前合成器给出的完整采集范围为 1868×1028；超过范围的请求明确拒绝。宿主保持比例，并显示实际接收像素、目标/放大比例；原始像素模式按 Retina backing scale 缩小内容区 | 仍未支持当前原生 Wayland Firefox 在大 Mac 窗口中的完整 Retina 分辨率。PNG 只消除压缩损失，不增加源像素；没有用放大冒充适配 |
| 应用入口与 Snap | 多个 Firefox 窗口时启动器要求从运行列表选择；新飞书首次超时明确提示等待，随后复用同一 PID/会话。真实 Snap 对照证明旧服务 `NoNewPrivileges=yes` 阻止 snap-confine，修正后版本探测成功 | 当前 Agent 仍以普通用户运行，保留 Snap 自身沙箱；没有新增远程命令执行接口 |

新 Agent PID 3032522，安装前备份位于 `~/.local/state/lan-remoteapp/upgrades/clarity-snap-20261022`。恢复 Firefox 后的 Agent 更新没有结束原 Firefox；新飞书启动后没有再次重启 Agent。Mac 完整应用已重新构建并运行。静止无损刷新及原始像素模式提交为 `be580d6`，Snap 启动修复为 `3394f21`。

本次完整 Mac 门禁、Clippy、Ubuntu 编码回归、Snap 对照、两个隔离 QUIC 节点及 Mac 混合 PNG/H.264 解码回归通过；详见[验证记录](validation.md#2026-10-22-清晰度与-snap-启动回归)。不代表真实双向发布、多显示器、分数缩放、长时间输入压力或原桌面飞书路径全部通过。Mac 本机共享仍为关闭状态，本轮未重新完成反向实机验收。

结构化结果及已部署二进制校验和见[激活后记录](validation-artifacts/2026-10-22-window-activation.json)。[激活前记录](validation-artifacts/2026-10-22-window-resolution.json)保留原始状态，两份记录不可混作同一次进程存活证明。

## 通用 Wayland 高 DPI 输出（后续改进）

GNOME 50 的 `RecordWindow` 在创建流时固定其主显示器的缓冲区大小。新的通用窗口路径先为租约创建临时高 DPI 输出，再把原窗口移入该输出后创建窗口流。应用收到 Wayland 输出密度变化，重新提交高分辨率缓冲区；不是缩放 PipeWire 截图。此逻辑不检查 Firefox、飞书或其他应用名，不修改浏览器配置、应用参数、物理显示器缩放或持久显示设置。

当前输出默认 2× 密度，4352×2432 固定采集包络为 Mac 的 4096×2160/800 万像素视口预算预留阴影空间，避免拖动时反复插拔输出。应用实际窗口仍按每次请求的像素尺寸重新布局。最多同时占用四个临时输出；每个租约只容纳自己的根窗口与显式 transient。GNOME 把无关新窗口放入当前焦点所在输出时，会将其归还来源显示器。

创建操作串行化，防止并发设备认领彼此的输出；租约消失、原窗口关闭、扩展停用会释放输出并恢复原几何。旧 Agent 保留会话后不销毁窗口后端，因此桥接层在三秒没有租约操作后撤回输出，把原窗口送回来源显示器；重连重新准备输出。应用进程和内容始终保留。该改进在独立 GNOME 中通过：实际 PNG 2940×1604/2940×1846，GTK 自身报告 scale=2，另一原生窗口 scale=1，物理显示器布局保持，两个并发租约隔离、越权拒绝、释放/空闲回收/重连、后续 H.264 960×540，以及 Xwayland 大窗口与坐标换算回归。混合 DPI 下 Xwayland 的协议像素与合成器坐标可能不同，验收使用实际客户区换算，不再假设二者单位相同。

上述为隔离验证，真实 Firefox 的部署与验收记录另行补充；原来记录的 1868×1028 上限属于升级前窗口流。

## 通用 Wayland 连续输入（后续改进）

旧实现把每次 `input.text.commit` 都转换为“改写剪贴板、等待 16ms、Ctrl+V”。隔离 GTK 输入框把读取剪贴板推迟 150ms 后，`about:newtab-0123456789` 可稳定出现漏字/重复；下一次提交会先覆盖上一次尚未读取的内容。此证据确认通用实现存在竞争，但不能据此断言此前真实 Firefox 自动化冒号缺失只有这一个原因。

新的提交路径通过 GNOME 的 Wayland text-input 接口直接传递控制端已完成组合的 UTF-8 文本；不使用剪贴板，不按应用名选择逻辑。主动粘贴保留原来的独立命令。快捷键仍使用经过 Portal 授权的 EIS 通道：文本提交前用 libei 的 ping/pong 确认此前事件已进入合成器，再等待 Clutter stage 完成本轮事件处理；提交后等待 text-input 的 done 事件发出。只等待 GLib idle 的中间版本在“全选后立刻输入”中仍有先后颠倒，已由该同步流程修复，没有添加固定逐字延时。

没有 text-input 焦点的输入框会在修改内容前明确拒绝，并提示主动粘贴整段文字；不会伪造 Unicode 按键，也不会静默退回旧的逐字粘贴。libei 同步需要 1.4 及以上版本，断开、暂停或同步超时均拒绝继续发送且不自动重试。X11 独立会话沿用原有输入队列，当前飞书的运行实例未切换实现。

窗口桥报告接口版本 2。新 Agent 遇到仍在内存中的旧扩展会提示保存工作后重新登录，不把新服务配旧扩展当成输入成功。GNOME 50 的模块导入在 Shell 生命周期内缓存，禁用/启用不能加载修改后的模块；因此本轮先构建和隔离验证，不能为了激活而结束当前 Firefox、飞书或旧 Agent。

依据：[libei 同步语义](https://libinput.pages.freedesktop.org/libei/api/group__libei.html)、[Mutter EIS 输入线程同步](https://raw.githubusercontent.com/GNOME/mutter/50.1/src/backends/meta-eis.c)、[text-input 提交与 done](https://raw.githubusercontent.com/GNOME/mutter/50.1/src/wayland/meta-wayland-text-input.c)、[GNOME 扩展模块生命周期](https://raw.githubusercontent.com/GNOME/gnome-shell/50.1/js/ui/extensionSystem.js)。最终门禁和未激活范围见验证记录；隔离输入框通过不等同于真实 Firefox 已验收。

## 同尺寸截图揭示的已有 Xwayland 缩放缺口

用户随后提供两张均为 2940×1660 的截图：远程 ChatGPT 标题显示接收 2940×1604、沿用应用缩放，文字和控件明显小于本机窗口。只读检查确认原应用 PID 3007634、可执行文件 `/usr/lib/chatgpt/ChatGPT`、Xwayland 窗口 50331652，合成器资源缩放为 1，启动参数没有 `force-device-scale-factor`。检查时该窗口已为 1920×968；截图中的历史接收尺寸不能当作此次现场测量。没有读取应用账号或聊天内容、改变缩放、操作窗口或重启应用。资源缩放和参数也不等同于对应用内部字号或页面缩放的精确测量。

代码证据：Mac 自动模式按内容区 points × backing scale 请求像素；已有 X11 窗口沿用原缩放，而 `Backend::set_scale_factor` 只记录协议值，不改变应用 DPI。因而增加到 Retina 像素尺寸并不能保证保留逻辑布局和控件视觉大小。例如 1470×802 points 的 2× 内容区，需要应用以相同逻辑尺寸绘制 2940×1604 像素，而不是把 1× 布局范围也扩大一倍。这与截图中控件接近缩小一半的现象一致。

尚未激活的私有高 DPI 输出只用于原生 Wayland：`ensureOutput` 对 X11 直接返回；它不能被报告为已解决这张截图。当前新独立飞书还使用精确应用入口匹配的启动参数，该成功不等于通用应用缩放已经完成。后续统一工作须同时覆盖逻辑尺寸、目标密度和应用重绘能力；已有 X11 缺少本实现可安全使用的逐窗口 DPI 重绘通道，应明确能力限制，不能修改全局桌面缩放或以截图放大冒充适配。此次仅完成诊断，保留用户暂缓切换和当前会话的决定。

## 2026-10-24：新独立 X11 会话的通用 DPI

Mac 在 Retina 屏幕上新启动远端应用时，现有启动请求会携带 2× 密度。Ubuntu Agent 现在对**新建的私有 Xvfb** 同时设置显示器物理 DPI 和 X11 `Xft.dpi`：1× 为 96，2× 为 192，并在窗口管理器与应用启动前发布。GTK 通过 `GDK_SCALE` 绘制 2× 像素，`GDK_DPI_SCALE=0.5` 抵消字体重复放大；Qt 读取 Xft DPI，启用 Qt 5 的高 DPI 行为，但不再叠加 `QT_SCALE_FACTOR`。这使同样的逻辑窗口可以由应用重排并绘制更多源像素，而不是让 Mac 拉伸旧截图。Mac 现有等比绘制和鼠标映射没有改动。

这个设置只属于该私有 X 显示器，不改 Ubuntu 登录桌面的全局资源、物理屏幕缩放或其他应用窗口。优先接管的**已有** GNOME Wayland/Xwayland 窗口及已运行的独立会话仍沿用其启动时的 DPI；X11 DPI 是应用启动时读取的资源，不能把已经运行的 ChatGPT 或飞书无损地重新渲染进新会话。原生 Wayland 继续使用自身的输出缩放路径，尚未因本项改动获得实机画质验收。

隔离 Ubuntu 实测使用同一 GTK 夹具，1×/2× 的窗口逻辑尺寸均为 600×400，X11 实际客户区为 600×400/1200×800 像素，Pango 逻辑字体高度均为 19；另一个 Xft 终端的 60×12 字符网格实测窗口从 604×232 增至 1144×460 像素。此测试证明 X11 DPI 影响应用绘制；未测真实 Qt/Electron 应用的缩放、Mac 端 H.264 解码画面或现有用户窗口。测试只运行私有 Agent/Xvfb，没有替换正式 Ubuntu 服务、注销或重启桌面。
