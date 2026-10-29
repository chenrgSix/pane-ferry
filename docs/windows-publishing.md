# Windows 当前桌面发布器（开发预览，待实机验收）

首版目标：Windows 当前已登录桌面中的普通权限应用窗口，经现有设备配对与 QUIC，在 Mac/AppKit 或 Ubuntu/GTK 中显示和操作。一个会话只发布一个窗口，同一会话只允许一个控制端。断线和关闭接收窗口保留源应用，再连接使用原 session ID；不重新启动应用，不重放输入。

2026-10-29 已在 Windows 电脑上完成构建、命名管道和配对 QUIC 的自动化测试，**尚未完成 Windows → Mac/Ubuntu 的真实桌面验收**。测试使用的合成桌面只能验证协议与会话行为，不能证明 Windows 采集、编码、输入或中文在真实应用中可用。

## 首版范围

- 复用 `paneferry-network` 的邀请、证书固定、设备路由、QUIC 控制与每窗口视频流。
- Windows 本机 IPC 使用只允许当前账户且拒绝远程客户端的命名管道；身份和配对文件使用当前账户私有 ACL。Mac/Linux 仍使用原有 Unix socket。
- `paneferry-windows` 同时管理本机发布与公共网络组件，默认不共享。通过 `serve --share` 或本机 `sharing on` 显式开启；远端无法修改共享开关。
- 从“运行中的窗口”选择当前可见、同一用户和登录会话中的窗口。应用安装目录为空，不提供远程启动任意程序、桌面模式或后台隔离会话。Windows 接收端与图形管理入口见 [Windows 控制端](windows-receiving.md)。
- 窗口列表提供基于可执行路径 SHA-256 的稳定 `application_key` 和 `auto_connectable`，供 Mac/Windows 工作台接管已有窗口、自动接入新窗口。排除工具窗口、不激活窗口和系统桌面，避免将托盘辅助窗口等后台表面当作可操作的应用窗口；应用标识只用于偏好匹配，接入仍核验具体窗口身份。
- 用 Windows Graphics Capture 按 HWND 采集，使用系统 Media Foundation 软件 H.264 编码器，最高 30 fps，输出现有 Annex B 协议。每次连接、尺寸变化和隐藏恢复重建编码流；首帧必须是关键帧。没有 PNG 视频回退，不需要 FFmpeg 或额外下载编码器。
- 发布服务在整个运行期间保留图形运行环境，避免短连接释放最后一个 MTA 后与 WGC 内部线程的清理竞争。尚无捕获帧时等待下一帧，不把空队列当作连接失败。
- 接收 Mac 的 `input.key/keysym` 和 Ubuntu 的 `input.key.logical`；中文在接收端组合后以 Unicode 提交。常见导航、功能键和快捷键映射已实现；OEM 标点按初始美式键盘布局处理，其他布局待验证。
- 通过窗口属性代次、PID、进程创建时间、账户与登录会话检查窗口身份，避免 HWND/PID 重用后接管新对象。源窗口退出后会话结束。
- 首版不传输跨设备剪贴板。`clipboard.remote-only` 能力让新版接收端的复制／剪切／粘贴快捷键只操作 Windows 本机剪贴板，不读取或上传接收端剪贴板。
- 标准用户权限运行；管理员程序、UAC 安全桌面、锁屏和注销不在支持范围。普通鼠标移动不抢焦点，目标处于后台、被遮挡或被模态弹窗禁用时忽略悬停输入；缩放也不激活源窗口。主动选择或点击窗口时尝试前台激活；若系统拒绝程序化激活，将这次主动操作以零位移鼠标事件提交给 Windows 输入队列，再有界重试激活，不发送额外按键或点击。获得焦点后等待实际窗口置前完成，每次点击仍核对落点，不允许点击遮挡它的其他窗口或未接入的模态弹窗；按键、文字和滚轮仍要求目标已在前台。不合并其他应用的输入队列，不修改系统前台限制设置。
- 多窗口、菜单、模态弹窗、受保护内容、休眠、RDP 切换、无显示器运行、长期性能与硬件编码均待扩展或验证。本机与远端共享同一个焦点、鼠标和键盘，不提供隔离桌面。

## 准备与构建

建议使用 Windows 11 x64、当前已登录且解锁的普通用户桌面。构建需要 Rust MSVC 工具链、Visual Studio C++ Build Tools 与 Windows SDK；脚本不会自动安装依赖、修改防火墙或开启共享。

在 Windows PowerShell / PowerShell 中，从仓库执行：

```powershell
./scripts/check-windows.ps1
```

产物是 `target/release/paneferry-windows.exe` 和 `paneferry-network.exe`。发布器已经在进程内调用公共网络服务，日常只需前一个 exe；后一个保留用于既有开发诊断。当前是未签名命令行开发包，不是安装器。添加的 Windows CI 工作流执行同一脚本；工作流文件存在不代表 CI 已运行或通过。

## 使用

在 Windows 打开记事本等测试应用，并保持窗口可见。在普通权限 PowerShell 中运行：

```powershell
.\target\release\paneferry-windows.exe serve --share
```

它默认监听 IPv4 UDP 42424，状态位于 `%LOCALAPPDATA%\PaneFerry`。在第二个终端生成邀请：

```powershell
.\target\release\paneferry-windows.exe invite
```

把输出中的 `invitation` 通过可信方式交给 Mac 或 Ubuntu，在现有设备管理中填写 Windows 的局域网 `IP:42424` 并配对。邀请五分钟有效、只使用一次，不要提交到仓库或日志。若 Windows 弹出防火墙提示，仅按实际使用的局域网配置允许访问；程序不会自行创建规则。

在 Mac/Ubuntu 选择 Windows 设备，刷新“运行中的窗口”，连接指定窗口。首版“已安装应用”列表为空是预期行为。接收端应使用本次更新构建的版本，以获得远端剪贴板快捷键和错误提示的适配。

本机停止／恢复共享：

```powershell
.\target\release\paneferry-windows.exe sharing off
.\target\release\paneferry-windows.exe sharing on
.\target\release\paneferry-windows.exe status
```

`Ctrl+C` 停止发布器时尝试释放注入的按键，并保留原应用。进程退出、系统重启后不保留发布会话，需要重新选择窗口；配对身份保存在状态目录中。修改端口使用 `serve --share --listen 0.0.0.0:42425`。隔离测试可对所有命令一致使用 `--state C:\absolute\test-state`，不能混用生产状态。

Windows 也可主动配对：`pair` 从 stdin 读取含 `address`、`invitation` 的 JSON；`revoke` 读取 `device_id`，`address` 读取 `device_id` 与 `address`。不要将邀请放在命令行参数中。

## 已执行与未执行的验证

2026-10-29 Windows → Mac 发布实机补充：用户已确认画面、前台切换和点击正常；新建普通窗口约 4 秒内被 Mac 自动接入。Windows 仍只发布普通独立窗口，所属弹窗、文件选择器及最小化窗口尚未覆盖，不能视为已与 Linux 的全窗口接管范围一致。控制端重新开启全窗口接管会重新导入此前关闭的接收窗口，这项修复需要更新控制端。

截至 2026-10-24，本机已完成的范围以本次验证记录为准：

- macOS 上运行 Rust 发布器集成测试，使用真实 QUIC 配对和合成桌面，覆盖窗口目录、启动请求去重、单控制端、两种按键协议、中文与 emoji 请求、不重放输入、断线释放、同会话重连、关键帧与依赖帧顺序、退出通知，以及拒绝未批准目标和未知字段。
- `cargo test --workspace`、启动器单元测试与生产构建、Mac 发布器与窗口宿主构建通过；实际 AppKit 输入与剪贴板顺序测试通过。新增远端剪贴板快捷键测试确认接收端不读取本机剪贴板。
- 完整 `scripts/check.sh` 首次运行在 AppKit 自动重连测试的窗口焦点断言处失败；没有修改断言，独立重跑该项通过。随后补跑余下 H.264 与原生传输测试均通过，包括真实 QUIC 传输到 AppKit 后的解码、同窗口／同会话恢复及离线输入丢弃。这不等于完整脚本一次运行全部通过。
- Ubuntu 接收端 Python 语法检查通过；新增 GTK 输入测试尚未在 Ubuntu 上运行。需要在 Ubuntu 验收快捷键、中文输入法与 Windows 实际画面。
- Windows MSVC 目标的 Rust 类型检查覆盖平台 IPC、传输控制客户端和原生窗口后端，包括 Windows API 绑定；这不是 Windows 可执行文件链接或运行验证。
- 当前 Mac 缺少 Windows C SDK，完整 Windows 网络/发布器的交叉构建在 `ring` 的 C 依赖处受阻；由 Windows 构建脚本和后续 Windows CI/实机完成。未以跳过加密依赖的方式宣称构建通过。

Windows 电脑准备好后的验收清单：

可在已解锁且有普通应用窗口的 Windows 桌面执行 `cargo test -p paneferry-backend-windows --test capture_lifecycle -- --ignored --nocapture`。它选择一个可自动接入的可见窗口，在内存中反复采集、编码并断开 10 次，不保存像素或发送输入。2026-10-29 本机执行通过；这不替代 Mac 端画面与输入验收。

真实输入验证使用 `tests/fixtures/windows-input.ps1 -ResultPath <临时文件>` 创建独立输入框。将该 PowerShell 进程 PID 和结果文件路径分别放入 `PANEFERRY_INPUT_FIXTURE_PID`、`PANEFERRY_INPUT_FIXTURE_RESULT`，执行 `cargo test -p paneferry-backend-windows --test input_activation -- --ignored --nocapture`。测试仅操作 PID 和固定标题均匹配的测试窗口，验证后台点击激活、输入框焦点及中文和 emoji 文本，不向其他应用打字。执行期间不要操作本机鼠标或键盘；测试窗口在 60 秒后关闭。2026-10-29 本机通过，Mac 到 Windows 的多窗口操作仍需实机确认。

遮挡与前台限制回归：另起一个进程运行同一 fixture 并加 `-Blocker`，将其 PID 设为 `PANEFERRY_INPUT_BLOCKER_PID`。测试先确认窗口被该独立进程遮挡、直接 `SetForegroundWindow` 被拒绝，然后验证点击激活和输入。设置 `PANEFERRY_INPUT_FOCUS_FIRST=1` 还会验证显式远端焦点请求，并确认它不移动本机鼠标；不设置则直接验证后台点击。临时阻挡窗口退出时释放其前台锁，不改持久化系统设置。

1. 运行 Windows 构建脚本；确认生成 exe 并通过真实 Windows 命名管道与合成发布器测试。
2. 分别与 Mac、Ubuntu 配对，从各自实际接收窗口看见 Windows 记事本的实时画面。
3. 点击、拖动、滚轮、按键按下／抬起、常见快捷键；本机移动焦点或出现遮挡时验证输入不会误发。
4. 中文输入法提交“中文输入🙂”、多行文本，确认预编辑留在接收端；原生复制粘贴只影响 Windows 剪贴板。
5. 暂停网络／关闭接收宿主后恢复原 session，确认文本和进程保留，首帧可解码，断线期间的输入不重放。
6. 窗口缩放、125%/150%/200% DPI、跨屏、最小化恢复，检查画面、点击坐标、关键帧与尺寸版本一致。
7. 关闭源窗口、销毁后快速重建窗口、锁屏、关闭共享、撤销配对，检查旧会话和输入立即失效。

参考系统接口：[WGC 指定窗口](https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createforwindow)、[Media Foundation H.264 编码器](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder)、[SendInput 权限边界](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)。

## Windows 管理入口

2026-10-29 开发变更增加了 Windows 图形管理入口，共用上述状态目录和服务。启动管理器默认不开启共享，可在“本机应用共享”中显式切换。管理器显示 Windows 的当前桌面支持范围，不显示 macOS 的录屏/辅助功能授权流程。发布器会过滤带 `PaneFerry.RemoteWindow.1` 属性的接收窗口，防止远程画面循环发布。

构建脚本 `scripts/build-windows.ps1` 将发布器打入管理器资源，并提供当前用户 NSIS 安装包构建配置；安装包存在配置不代表已完成安装、升级和实机验收。
