# 交付验证记录

日期：2026-10-20。执行环境：助手云端 Linux；未连接用户 Mac 或 Ubuntu 进行开发/测试。用户已允许先不做实机测试。

## 已执行并通过

| 检查 | 结果 | 范围 |
| --- | --- | --- |
| `cargo build -p remoteapp-agent` | 通过 | Linux Agent 开发构建，包括 x11rb 与 PNG 代码编译 |
| `cargo test --workspace` | 18 项通过、0 失败 | 协议 8、transport 5、Agent 2、capture 2、core 1；不是 18 个端到端场景 |
| `cargo fmt --all -- --check` | 通过 | 根 workspace Rust 格式 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | 根 workspace 静态检查；Tauri 原生 crate 不在此 workspace |
| `npm run build` | 通过 | Vue TypeScript 类型检查及 Vite 生产构建 |
| shell 语法检查 | 通过 | check.sh、build-macos.sh、build-app.sh |
| Python 语法编译 | 通过 | 源码打包与 socket smoke 测试脚本 |
| 跨端协议人工复查 | 已完成 | version、session 字段、固定 SSH 命令、PNG 长度与几何版本；不替代真实执行 |

Rust 测试覆盖包括：拒绝未知消息/字段/版本、整数版本、尺寸上限、超长/截断消息、原生输入契约、应用/主机参数注入、固定 SSH 入口、Xpra 附加功能关闭、PNG 头与短图像输入。

## 受限/未执行

- **Tauri 原生构建未通过环境准备阶段**：云端缺少 `glib-2.0 >= 2.70` 开发库，`glib-sys` 的 pkg-config 检查阻塞。没有擅自安装系统依赖。前端构建通过不代表 Tauri 原生应用构建通过。
- **Swift/AppKit 未编译**：当前 Linux 环境无 macOS SDK/Swift。已有 Swift Package、AppKit 源码和 macOS 构建脚本；仍可能存在需在 Mac 编译时修复的问题。
- **Unix socket smoke 未通过运行**：沙箱不允许所需 socket 操作，Agent 启动受阻，未实际执行连接断言。保留 `tests/e2e/agent_socket_smoke.py` 供允许 Unix socket 的开发环境运行。
- **X11/Xpra GUI 未运行**：云端没有 Xvfb/Openbox/Xpra 运行依赖。没有宣称真实 PNG 捕获或输入成功。
- **未做真实 Ubuntu 26.04 → Mac 联调**，未测延迟、Retina、中文输入、复杂菜单/弹窗、重连矩阵、休眠或 GPU 应用。
- 未对 UI 做浏览器或原生视觉验收；未做外部安全审计、签名或公证。

## 交付结论

这是可继续构建的 **P0/P1 开发者源码预览**，不是已验证“Linux 应用现在就能在你的 Mac 使用”的成品，也不是原设计全部 V1/V1.5/V2 能力已完成。使用前需在目标平台安装前置依赖、构建并按 `docs/acceptance.md` 验证。

当前实际技术：SSH + JSON Lines + PNG，X11 GetImage 轮询，手动批准 JSON 应用目录，Swift NSView/CoreGraphics。Protobuf schema 为后续迁移草案；未实现 QUIC、H.264、Metal、完整 IME、剪贴板同步、Retina 远端重绘、自动发现、独立 Dock 或文件拖放。
