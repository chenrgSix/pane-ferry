# 交付验证记录

日期：2026-10-20。执行环境：Apple Silicon Mac（macOS 27.0.1）与局域网 Ubuntu 26.04.1 x86_64。当前只保留自有原生窗口链路。

## 本地构建与检查

| 检查 | 结果 | 范围 |
| --- | --- | --- |
| `cargo test --workspace --locked` | 22 项通过、0 失败 | 协议 11、transport 5、Agent 2、capture 2、core 2；不是 22 个端到端场景 |
| `cargo fmt --all -- --check` | 通过 | 根 workspace Rust 格式 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | 根 workspace 静态检查 |
| `npm test` | 3 项通过 | 待确认启动请求的幂等 ID、持久保存与容量限制 |
| `npm run build` | 通过 | Vue TypeScript 类型检查及 Vite 生产构建 |
| `tests/e2e/ssh_helper_validation.py` | 11 个非法参数案例通过 | SSH 登录辅助脚本参数校验 |
| `swift build` 与短消息 pipe smoke | 通过 | AppKit 编译；短 SSH 消息无需等待完整缓冲区、EOF 与分段读取 |
| `tests/e2e/controller_input_smoke.py` | 通过 | 真实 NSTextInputClient 预编辑不出控制端、Unicode 提交、按需复制/粘贴、大小边界与回包关联；测试不触碰系统剪贴板 |
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

现有窗口接入版本曾接入 Ubuntu 原桌面正在运行的 ChatGPT（同一进程、同一登录状态），验证数字输入、清空草稿与重连，未发送聊天消息。新增输入法和剪贴板能力后的联调尚未完成：先后遇到桌面锁屏、原桌面上的 Xwayland 系统授权弹窗，以及授权后仍不响应点击和输入的问题。用户正在检查原应用是否也无响应；已按要求停止测试 Agent 并关闭临时 SSH 连接，保留原 ChatGPT 进程。不能将之前的基础输入测试当作新版剪贴板已经在 ChatGPT 验收通过。

`tests/e2e/existing_window_smoke.py --controller-text` 已在临时 HOME、自建 Xvfb/Openbox 与 GTK TextView 上验证按顺序提交中文/emoji、控制端粘贴、多行 UTF-8、主动复制回传、无选中文字时拒绝返回旧剪贴板，以及超时后继续输入。后续一次回归在原有 xterm 输入测试未通过，需排查焦点/就绪时序；独立 `native_session_smoke.py` 的真实键鼠、缩放、隐藏及 3 次重连仍通过。现有应用的 Xwayland 授权与兼容性需要独立验证。

## 尚未覆盖

- 当前覆盖 xterm 基础链路和现有 ChatGPT 的画面、输入与重连；未完整验证复杂 GTK/Qt 应用、菜单树、透明窗口、GPU 程序及 Wayland 原生程序。
- 输入法基础预编辑/提交和主动双向纯文本剪贴板已实现并有独立测试；复杂中文输入法、精确候选位置、现有 ChatGPT 往返、图片/文件剪贴板仍未完成实机验收。后台剪贴板同步不在当前范围。Retina 远端重排、双屏、休眠与网络切换、生产性能和长时间稳定性尚未覆盖。
- 未做签名、公证、外部安全审计、CI 或生产验收。Ubuntu 联调使用临时用户服务，尚未验证重启系统后的自动启动。
- 会话与幂等表在内存；停止 Agent 会结束其子进程，不能将断线恢复通过解释为 Agent/系统重启后仍能恢复。

当前技术为 SSH + JSON Lines + PNG、X11 GetImage 轮询和 Swift AppKit/CoreGraphics。Protobuf schema 为后续草案；QUIC、H.264、Metal、精确远端文本上下文与文件拖放未实现。
