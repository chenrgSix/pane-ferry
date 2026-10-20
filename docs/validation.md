# 交付验证记录

日期：2026-10-20 至 2026-10-21。执行环境：Apple Silicon Mac（macOS 27.0.1）与局域网 Ubuntu 26.04.1 x86_64。当前只保留自有原生窗口链路。

## 本地构建与检查

| 检查 | 结果 | 范围 |
| --- | --- | --- |
| `cargo test --workspace --locked` | 23 项通过、0 失败 | 协议 12、transport 5、Agent 2、capture 2、core 2；不是 23 个端到端场景 |
| `cargo fmt --all -- --check` | 通过 | 根 workspace Rust 格式 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | 根 workspace 静态检查 |
| `npm test` | 3 项通过 | 待确认启动请求的幂等 ID、持久保存与容量限制 |
| `npm run build` | 通过 | Vue TypeScript 类型检查及 Vite 生产构建 |
| `tests/e2e/ssh_helper_validation.py` | 11 个非法参数案例通过 | SSH 登录辅助脚本参数校验 |
| `swift build` 与短消息 pipe smoke | 通过 | AppKit 编译；短 SSH 消息无需等待完整缓冲区、EOF 与分段读取 |
| `tests/e2e/controller_input_smoke.py` | 通过 | 真实 NSTextInputClient 预编辑不出控制端、Unicode 提交、按需复制/粘贴、大小边界与回包关联；像素/点数换算、自动铺满与固定等比模式的点击；AppKit event-tracking 模式中的尺寸同步、迟到几何回包隔离及手动/自动切换；测试不触碰系统剪贴板 |
| `tests/e2e/hidpi_smoke.py`（Ubuntu） | 通过 | GTK 1×/2× 实际缩放、逻辑尺寸保持、PNG 像素翻倍、EWMH resize 与输入；WM/未显示辅助窗口不发布 |
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

## 尚未覆盖

- 当前覆盖 xterm、独立 GTK 编辑器及原桌面 Firefox 的基础操作；现有 ChatGPT 的新版联调尚未完成。未完整验证复杂 GTK/Qt 应用、菜单树、透明窗口、GPU 程序及 Wayland 原生程序。
- 输入法基础预编辑/提交和主动双向纯文本剪贴板已实现并有独立测试；复杂中文输入法、精确候选位置、现有 ChatGPT 往返、图片/文件剪贴板仍未完成实机验收。后台剪贴板同步不在当前范围。GTK 2× 重绘已验证；跨屏幕 DPI 热切换、Qt/传统 X11 应用、双屏、休眠与网络切换、生产性能和长时间稳定性尚未覆盖。
- 未做签名、公证、外部安全审计、CI 或生产验收。Ubuntu 联调使用临时用户服务，尚未验证重启系统后的自动启动。
- 会话与幂等表在内存；停止 Agent 会结束其子进程，不能将断线恢复通过解释为 Agent/系统重启后仍能恢复。

当前技术为 SSH + JSON Lines + PNG、X11 GetImage 轮询和 Swift AppKit/CoreGraphics。Protobuf schema 为后续草案；QUIC、H.264、Metal、精确远端文本上下文与文件拖放未实现。
