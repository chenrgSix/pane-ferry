# 窗口像素、界面缩放与 Wayland

本页区分已定位的问题、代码行为和实机验收。Firefox 的目标是连接原窗口、账号和标签页；飞书目标是窗口重新布局后采集原始像素，不把低分辨率截图拉伸为目标尺寸。

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

清晰度还受两个独立因素影响：低分辨率画面在 Retina 内容区放大，以及 H.264/I420 对文字和彩色边缘的有损压缩。静止后不再只保留最后一张视频帧：支持该能力的 Mac 接收端会在窗口停止变化约 250ms 后收到原始像素的 PNG 无损刷新；继续变化时恢复原 H.264 流。这不会制造更多源像素，也不会改变应用的界面缩放。

Mac 标题会显示实际画面的放大百分比；“显示 → 原始像素大小”按当前屏幕 backing scale 调整内容区，使一个源像素对应一个屏幕像素，并固定当前实际分辨率。该选项可用于辨认放大造成的模糊；1× 远端应用的控件在 Retina 上可能偏小。需要大窗口和合理控件大小时仍须真正提高远端渲染像素并配置应用缩放，不能以无损编码替代分辨率适配。

## 已有窗口与新独立实例

点击应用入口先查同一应用的已有会话，再查原桌面窗口，最后才新建实例。有多个候选要求选择，有控制端则报告占用。十秒内没有可见窗口返回待确认并保留请求 ID，提前退出明确失败，不把退出码 0 当成已经有画面。

已有飞书仍由其原工具包决定 UI 缩放。已检查安装包装器会转发参数，渲染库支持 Chromium 的 `force-device-scale-factor`；仅对准确匹配的飞书桌面入口的新独立实例添加该参数，保留用户显式参数。GTK/Qt 环境变量不是飞书缩放已生效的证据。真实飞书 2× 视觉效果没有验收，因为没有为测试退出原应用；需要重启时须另行保存工作并明确执行。

Firefox 接管只使用合成器识别的原 PID / 窗口，不创建或复制浏览器配置。无法获得原窗口时明确提示，不自动改用隔离资料。新 Wayland 路径和原 X11 路径共用 Agent、QUIC、会话控制和设备隔离。

## GNOME 50 采集与尺寸限制

`paneferry@local` 扩展提供窗口列表、目标租约和尺寸操作。租约限定本地 D-Bus owner、原窗口 ID/PID、明确 transient 链；列表只读取元数据。关闭连接或锁屏不结束原应用；其他设备和其他主窗口不能借此扩大控制范围。

- **原生 Wayland**：Mutter RecordWindow 提供 PipeWire 流。GNOME 50 的窗口流采用显示器大小的缓冲区，实际区域由 crop 元数据描述。GStreamer 直接提取原始 BGRx 区域并去除阴影，不先做 videoconvert 丢掉 crop，也不通过缩放制造目标像素。超出完整采集范围明确返回 `resolution_limited`。该路径暂不支持超过显示器采集范围的原生窗口。
- **Xwayland**：保留 XComposite 像素采集，用合成器 `move_resize_frame(true, …)` 处理明确的用户尺寸请求，区分客户区、装饰和合成器坐标。窗口可比原显示器更大；输入先校验实际客户区尺寸，仅移动被选窗口，使对应点处于可输入范围。该移动会体现在 Ubuntu 本机桌面。
- **输入和剪贴板**：键鼠仍须系统 RemoteDesktop Portal 授权。原生 Wayland 当前只有文本复制/粘贴，剪切在删除前拒绝，图片/文件剪贴板明确不支持。实际原生 Firefox 的键鼠与剪贴板还未验收。

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
