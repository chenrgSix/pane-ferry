# Windows 控制端（源码开发预览）

Windows 管理界面复用 macOS 的 Vue/Tauri 工作台：设备邀请和配对、应用目录、运行中的窗口、远程会话、自动接入和本机共享。网络与 Windows 被控端共用 `%LOCALAPPDATA%\PaneFerry` 中的身份、配对和私有命名管道。

接收窗口使用 Tauri 原生窗口与 WebView2 WebCodecs 解码 H.264，复用现有配对 QUIC 与 RWP/1。显示和关闭行为参照 AppKit 控制端，但不是 AppKit 窗口宿主的完整移植。

## 构建和启动

需要 Windows 11、Rust 工具链、对应 C/C++ 工具链、Node.js/npm 和 Microsoft Edge WebView2 Runtime。常规发布使用 MSVC + Visual Studio C++ Build Tools + Windows SDK。

```powershell
./scripts/build-windows.ps1
```

脚本执行 Windows 发布器测试、构建发布器、运行前端测试并生成 Tauri/NSIS 当前用户安装包。输出位于 `apps/launcher/src-tauri/target/release/bundle/nsis/`。只构建可执行文件可用 `-NoBundle`；发布器需与管理器资源一同部署。

启动管理器后，在“设备连接”生成邀请或填写另一设备的邀请。选择设备，读取应用或运行窗口，点击连接。原命令行 `paneferry-windows serve` 与图形管理器共享本机服务，不要用不同副本同时占用同一状态和端口。

## 已实现的行为

- 每个远端会话对应一个 Windows 接收窗口；同一设备和会话重复打开复用现有窗口。
- H.264 Annex B 流按顺序解码，首帧检查 SPS/PPS/IDR；校验尺寸、几何版本、编码流和参考内存预算。慢解码对上游施加背压，不直接丢弃依赖帧。
- 等比显示，鼠标坐标只映射到图像区域；黑边不触发远端点击，拖动越界时限制到源窗口边缘。
- 自动适应本机内容区、保留远端尺寸，以及 1280×720 / 1920×1080 固定尺寸。使用逻辑尺寸，避免 Windows DPI 重复乘入远端坐标。
- 30/60 fps 选择受发布端能力限制。切换通过重连原会话重新协商；Windows 发布器目前最高 30 fps。
- 鼠标、拖动、滚轮、导航键、快捷键和中文/emoji 文本提交。输入法预编辑保留在本机；Mac 被控端使用逻辑按键协议。
- 断线清空输入队列与解码器；恢复同一 session，输入不重放。失焦释放已按下的键和鼠标按钮。
- 关闭接收窗口仅关闭控制连接，不发送关闭远端应用的请求。接收窗口带有本机属性标记，Windows 发布器会排除它，避免再次转发。

## 当前边界与验收

目前多成员会话通过窗口顶部列表切换查看，未实现 AppKit 的菜单、弹窗叠放及每应用任务栏分组。剪贴板快捷键只操作远端剪贴板，尚未实现跨设备文本、图片和文件传输。桌面模式仍取决于远端发布器能力。Windows 被控端仍只发布当前桌面的已存在窗口，不支持已安装应用启动和整桌面发布。

自动化测试不替代跨设备实机验收。需要分别连接 Mac 与 Ubuntu，验证实际 H.264 画面、点击拖动、快捷键、中文输入法、125%/150%/200% DPI、窗口调整、断网重连、撤销配对和共享关闭。NSIS 安装、升级、卸载及发布签名也需单独验收。

实现参考：[Tauri 前端通信](https://v2.tauri.app/develop/calling-frontend/)、[WebCodecs H.264 Annex B 注册](https://www.w3.org/TR/webcodecs-avc-codec-registration/)。

## 2026-10-29 本机验证记录

- Windows GNU Rust 工具链完成 `scripts/check-windows.ps1`，包含发布器/平台测试、真实配对 QUIC 集成测试、网络组件测试及发布器 Release 构建。
- Tauri 控制端 `cargo check`、20 项 Rust 测试、Debug 与 Release 的 `tauri build --no-bundle` 均通过。Release 入口为 `apps/launcher/src-tauri/target/release/paneferry-launcher.exe`，同目录包含发布器资源。接收端新增测试通过真实 Windows 命名管道验证会话绑定、帧率上限、窗口隔离、取消静止连接、拒绝失效连接输入，以及关闭时不发送关闭远端应用请求。
- 53 项前端测试与生产构建通过；新增测试检查真实 H.264 SPS、关键帧/P 帧、尺寸、黑边/拖动坐标、源端缩放和 Mac 逻辑按键。
- 在本机内置 Chromium 浏览器运行 `npm run dev` 后打开 `/tests/receiver-smoke.html`，使用生产接收代码与模拟 IPC，实际解码 H.264 并检查红色像素；中文/emoji 只提交一次、失焦按键释放、重连期间输入丢弃通过。这不是完整 QUIC 到 WebView2 的跨设备验证。
- Windows 控制端 exe 已实际启动并显示工作台；首次启动出现 Windows 防火墙提示，未自动修改防火墙授权。Mac/Ubuntu 跨设备、真实 WGC 采集、NSIS 安装升级及 MSVC 发布构建仍待验收。
