# ChatGPT 单窗口页面高清（2026-10-24）

已接入显式的 2× 页面适配：选中现有 ChatGPT XWayland 主窗口，在 Mac WindowHost 的“显示 → ChatGPT 页面高清（2×）”开启。源窗口扩大为两倍像素，Mac 保持原内容区点数；再次点击、断开连接或发布器退出后恢复。默认不启用，也不在重连时重新开启。

这是当前 ChatGPT 布局的专用适配器，不是通用 Electron `setZoomFactor`。当前运行时的原生缩放不能提供已验证的同源窗口隔离，所以不调用该接口。WebKit 与 GTK 路线没有加入本次实现。

## 准备与范围

需要当前用户的 `/usr/lib/chatgpt/ChatGPT`、源端密度 1×、系统 Python 3 + python3-websocket、libX11 和现有 GNOME XWayland 接管能力。Ubuntu 本地运行：

```sh
paneferry-agent prepare-chatgpt PID
```

已存在合格回环调试入口时不重启；否则只接受没有额外启动参数的普通实例，以 SIGTERM 退出后使用原 profile 和桌面环境重启，等待主进程的回环调试端口就绪。不强杀或复制 profile。准备动作会重启整个 ChatGPT 进程；此后高清租约只作用于选中的主窗口。此命令不属于远程协议。

适配器从真实用户的 `.config/Codex/DevToolsActivePort` 读取端口，核验监听 socket 属于已授权主 PID，且仅绑定 IPv4 回环、没有同端口 IPv6 监听。CDP 端口不传给控制端。调试端口提供应用控制能力，因此准备是用户显式操作，保持仅本机可达。

在同 UID、可执行路径、进程启动时间、X11 窗口归属检查之后，再要求唯一的 `app://-/index.html` 顶层页面与 X11 客户区位置、尺寸完全匹配。两扇同源主窗口完全重叠时拒绝操作；错开位置或尺寸后才能开启。只有标题相同不能构成匹配证据。

## 布局与恢复

主页面使用应用已有的 `--codex-window-zoom` 布局变量，将原有效值乘 2。应用已有的视口换算继续生效；直接给根节点设置 `zoom:2` 会使本版本的嵌入浏览器浮层位置重复放大，因此未采用。浏览器 WebView 位于不缩放的浮层中，另用临时 CSS 设置元素的 zoom=2 和 width/height=100%，通过正常布局获得 2× 客体 DPR，不修改客体页面或同源全局缩放。

只支持识别出的应用布局及固定浮层中的浏览器 WebView。出现未知可见嵌入布局时拒绝/结束租约；应用升级后可能需要重新验证。登录后的主界面、正文、浏览器和 iframe 已实测。部分提示浮层仍可能保持原字号；原生窗口装饰、系统菜单、文件对话框不属于页面高清覆盖范围，不能把此功能称作整个应用的原生 DPI 适配。

私有辅助进程保存 CSS 变量原值、优先级及原本缺失状态，保存目标的实际位置、尺寸、最大化/全屏状态，并用窗口属性标记本次归属。它在控制连接 EOF、父进程退出或 SIGTERM 时独立恢复。测试发现 systemd 会在发布器被杀死后终止同组辅助进程，因此辅助进程显式处理终止信号，先执行恢复。CSS 另有 4 秒心跳到期恢复，不累乘倍率。

若辅助进程自身被 SIGKILL、整个桌面退出或应用销毁，无法保证恢复原窗口几何；CSS 心跳只有页面仍运行时才有效。恢复失败会明确记录，不宣称完整恢复。最大化/全屏快照逻辑已实现，但本轮实机采用普通窗口，未验收这些状态、多屏迁移或长期性能。

协议新增受限的 `window.page-scale {window_id,scale:1|2}`，不接受 URL、代码或任意进程信息。源端 `scale_factor` 保留实际值 1；`page_scale_factor` 和 `presentation_scale_factor` 单独表示租约及展示密度，避免把源端倍率伪造成 2。其他主窗口和关联弹窗不继承页面租约。

## 实测证据

测试使用原桌面的真实登录 profile，ChatGPT 包版本 26.1013.40934；Chrome 155.0.8059.80。授权重启一次后登录保留，后续试验保持 PID 820738 / start_ticks 7862967 / XID 52428804。没有修改 app.asar 或应用可执行文件，没有发送聊天消息。另建同进程主窗口作为旁观者，试验完成后关闭。

| 检查 | 实际结果 |
| --- | --- |
| Codex 模式登录界面及嵌入页面 | 原图 1470×813 → 2940×1626；WebView DPR 1→2，逻辑视口维持 554×665 |
| ChatGPT 模式登录界面及嵌入页面 | 原图 1470×805 → 2940×1610；WebView DPR 1→2，逻辑视口维持 554×657 |
| 真实输入与导航 | 经 QUIC、Agent 和已授权 Portal 点击计数按钮，提交“窗渡缩放验证”，再导航至夹具 `/next`；iframe DPR 2、宽度保持 320 |
| 同进程旁观窗口 | 位置 (50,50)、尺寸 900×650、页面变量和 DPR 全程保持基准 |
| 强制关闭 QUIC 桥接进程 | 目标原 CSS 缺失状态、位置和尺寸恢复；浏览器 DPR 回到 1 |
| SIGKILL 发布器主进程 | systemd 随后终止辅助进程，辅助进程完成 CSS 与几何恢复；旁观窗口未变 |
| 真正的 AppKit 宿主 | 菜单开关、H.264 解码 1470×805→2940×1610；同一个 NSWindow 的 frame 不变；显式关闭及自动重连恢复 1× |
| 重叠窗口歧义 | 返回 `page_scale_target_ambiguous`，未改变缩放 |

结构化结果见 [验证记录](validation-artifacts/2026-10-24-chatgpt-page-scale.json)。已查看 XComposite 原始截图和 AppKit 解码图片；包含个人界面的截图仅留本机私有临时目录，不加入仓库。表中尺寸来自实际窗口、客体和捕获/解码结果，不是仅凭请求值。

完整 `scripts/check.sh` 通过：Rust 63 passed / 6 ignored、前端 32 passed，及现有 Swift/AppKit、剪贴板、H.264、QUIC 和重连门禁。后续增加 SIGTERM 恢复修复后，重新通过三项 Python 专项测试、Ubuntu 构建和上述同场景实测。Mac 与 Ubuntu release 组件已构建；这不代表 CI 或生产升级验收。

## 复验与交付

无需桌面和账号的恢复/范围测试已纳入 `scripts/check.sh`：

```sh
python3 tests/e2e/electron_page_scale_unit.py
```

真实 AppKit/QUIC 测试是显式 opt-in；先在测试 Agent 接管已准备的目标窗口，再传入该会话及已配对路线。测试会按正常宿主行为调整源窗口尺寸，开启/关闭高清、断开并重连，最后 detach，不关闭远端应用：

```sh
python3 tests/e2e/live_page_scale_smoke.py \
  --session SESSION_ID --native-peer DEVICE_UUID \
  --native-state /absolute/private/network-state \
  --native-helper /absolute/path/paneferry-network
```

`tests/e2e/page_scale_fixture.html` 是嵌入浏览器的本地夹具；在目标浏览器中打开本地 HTTP 地址后，可核验按钮、中文输入、同源导航和 iframe。真实系统输入仍需用户在 Ubuntu Portal 授权，不能用 CDP 点击冒充 Portal 输入通过。

本轮使用独立 Agent/QUIC 路线验证，正式 Agent 和网络服务未切换。正式 Agent 保留四个其他应用会话，重启会结束这些独立应用；这不符合“只影响目标窗口”的边界。构建产物可供升级，不能把独立验证实例通过写成正式服务已升级。ChatGPT 保持已准备的回环调试启动方式；关闭后普通启动会再次需要准备。
