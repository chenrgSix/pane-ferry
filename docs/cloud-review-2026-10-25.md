# PaneFerry 云端代码审查与修复（2026-10-25）

> 下文保留云端交付时的审查结论及其环境限制；本地合入后的实际结果见末节“本地集成与补验”。原报告中的“未运行”描述云端当时的状态。

## 范围与状态

- 仓库：`https://github.com/chenrgSix/pane-ferry`
- 基准：`006615a96035497d9b899e2e1578bfa0b1850529`。本次在独立云端检出上修改，没有推送、合并、部署或修改用户电脑。
- 检查了 Rust Agent/窗口生命周期/采集编码队列、QUIC/协议/传输、GTK 接收管理、Tauri/Vue 状态流、macOS 发布与接收边界、Windows 发布状态机。已有历史验收记录仅作背景，不算本次测试结果。
- **这是待原生验证的源码补丁，不是已验收的安装包。** 本环境没有 Cargo/rustc、Swift/macOS SDK；Rust 构建、Rust 新回归与 Swift 新回归尚未执行。前端和可独立运行的 Python/JavaScript 门禁已执行。

## 已修改的缺陷

| # | 优先级 | 用户表现与代码证据 | 修复与回归 |
|---|---|---|---|
| 1 | P1 | GTK `network_job` 将 `refresh_peers` 注册为 GLib idle，而该函数返回 True，导致每次操作叠加无限刷新/文件读取 | 单次回调返回 False；周期刷新独立为 `poll_peers`。`linux_manager_unit.py` 已通过 |
| 2 | P2 | GTK 管理窗口关闭后重开，旧 `peer_keys` 缓存仍命中，新下拉框不填设备；旧定时器也可能继续存在 | 新控件重置缓存，destroy 移除轮询。管理器回归已通过 |
| 3 | P1 | 同一设备同一会话连续点击会创建多个接收器，竞争单控制端并报错 | 以 `peer_id + session_id` 复用存活接收器；不同设备不混淆。管理器回归已通过 |
| 4 | P1 | Vue 旧 `local_node_info` 请求晚于“停止共享”返回，界面又显示共享开启，下一次操作方向错误 | 读请求代次隔离、变更期间排除轮询及重复提交。两个新增生产 App.vue 回归修复前失败、修复后通过 |
| 5 | P1 | bulk 文件操作的连接、等待 QUIC 流额度及写请求头在原来的 body timeout 外，已配对但不返还流额度的设备可永久卡住操作 | setup 统一绝对截止时间。新增真实 QUIC 零流额度测试，**未运行** |
| 6 | P1 | QUIC 只保证单流顺序：新媒体流 IDR 已收到后，旧流残留 P 帧仍可通过窗口/几何校验进入解码器 | 接收侧记录真实 QUIC stream ID，拒绝更旧媒体流；不改变线上消息格式。新增交错代次回归，**未运行** |
| 7 | P2 | Linux QoS 用最后一次反馈的绝对年龄判定阻塞，长时间静止后首帧尚未有机会反馈就被降帧率 | 还必须有至少一秒年龄的未确认帧。新增“静止 30 秒后恢复”和真正超时对照，**未运行** |
| 8 | P1 | Linux 输出及编码原始帧队列在像素预算满时丢弃其他窗口的最后一帧；采集已确认 XDamage，静止窗口可能永远不再重采集 | 保留不同窗口的先到任务；被拒绝的最新原始快照产生按窗口重采集通知（最多 64 个 ID）；主循环重置该窗口 dirty，并优先采集上轮被拒绝的窗口。原始队列预算不变，已编码 H.264 帧不丢弃。两个队列压力回归和重试窗口优先顺序回归已加入，**未运行，需重点原生压力验证** |
| 9 | P2 | `desktop.start` 重放历史请求不检查 exited；其名额判断也没有像应用启动一样回收已退出会话 | 桌面与应用共用已退出会话回收；历史请求保留并明确返回 `session_exited/session_expired`。扩展会话回归，**未运行** |
| 10 | P1 | Windows 窗口在 geometry() 与 frame() 之间正常缩放，旧几何与新帧不一致会触发 `capture_failed` 并断开 | 放弃该竞态帧、重置编码关键帧，下一 tick 先发送正确元数据。模拟 WindowSystem 回归已加入，**未运行** |
| 11 | P1 | Windows 两个客户端同 request_id、不同 app_id 并发启动，在解锁检查几何后第二次上锁直接复用请求，可能返回错误应用会话 | 两个临界区都验证 request→app 绑定；确定性交错回归已加入，**未运行** |
| 12 | P1 | Mac 接收端拖拽时远端缩小，失焦释放仍发送旧越界坐标，发布端可能拒绝 mouse-up，留下按键状态 | `releaseInput` 把释放坐标限制到最新尺寸；真实 SurfaceView 输入回归已加入，**未运行** |
| 13 | P2 | Mac 发布端 NSNumber.intValue 将 1.5/1.9 协议版本截断为 1；分块读取也可能接受超过总行长上限的 inline paste | 精确版本判定，解析前再次检查完整行长；有效/无效版本及超大行回归已加入，**未运行** |

## 本轮已执行的验证

- `cd apps/launcher && npm test`：36/36 通过；最终集成后复跑通过。
- `cd apps/launcher && npm run build`：vue-tsc 与 Vite 生产构建通过；最终集成后复跑通过。
- `python3 tests/e2e/linux_manager_unit.py`：通过（单次/周期回调、窗口重开、定时器清理、按设备会话去重）。已加入 `scripts/check.sh`。
- `python3 tests/e2e/linux_runtime_unit.py`：通过。
- `python3 tests/e2e/linux_wire_unit.py`：通过（空闲连接、请求期限、部分包体期限）。
- `python3 tests/e2e/linux_receiver_boundaries.py`：通过。
- `python3 tests/e2e/linux_install_safety.py`：通过，仅在临时目录与模拟 systemctl 上执行。
- `python3 tests/e2e/desktop_authorization_unit.py`：4 项通过。
- `python3 tests/e2e/desktop_monitor_unit.py`：4 项通过。
- `python3 tests/e2e/wayland_capture_unit.py`：通过。
- `python3 tests/e2e/electron_page_scale_unit.py`：5 项通过，附带 JS scope 回归通过。
- `node tests/unit/application_scene.mjs`、`virtual_output_lifecycle.mjs`（11 场景）、`window_presentation.mjs`：通过。
- `python3 -m compileall -q apps/linux-agent/src apps/linux-client tests/e2e`：通过。
- `git diff --check`：通过。

### 未运行或受阻

- Cargo/rustc/rustfmt 不存在。因此 `cargo fmt --all -- --check`、`cargo test --workspace`、Clippy 与所有新增 Rust 回归均未执行；不能声称 Rust 可编译。
- `tests/performance/scene_probe_unit.py` 已尝试，因缺少 `swiftc` 退出 1，尚未进入其测试。
- 没有 macOS SDK：Swift/AppKit/VideoToolbox 原生构建与测试未运行。
- 当前没有 DISPLAY/Xvfb，且系统 Python 缺少 Gst namespace：真实 GTK/GStreamer/窗口采集未运行。
- Windows 原生 WGC/Media Foundation 与设备双机 QUIC 桌面交互未运行；未验证实际端到端延迟/画质。
- Tauri 原生构建与 aggregate `scripts/check.sh` 未运行，前端通过不等于原生启动器通过。

## 交付前/合入前建议验证

1. 有 Rust 工具链后先运行格式化、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`，并单独运行 Tauri workspace 门禁。
2. 在 Linux 隔离 Xvfb/GStreamer 环境验证多个大尺寸静止窗口并发、编码线程延迟、窗口销毁/重建及 resize；特别观察 #8 的首帧恢复、公平性、内存上限与事件先后顺序。
3. 在 macOS 跑现有 `scripts/check.sh`，包含新增 Wire 与输入边界测试；在 Windows 跑 `scripts/check-windows.ps1` 与实际 resize 拖拽。
4. 两台设备回归失联重连、流控饱和、停止共享、大文件取消、同会话重复打开与多窗口。没有真实双机证据前，不宣称所有问题已清除。

## 仍应单独跟进

- 部分旧架构/传输文档还描述早期“媒体失败终止整个会话”和终身窗口 ID 限制，与现有恢复实现不完全一致。此次没有把文档中的旧承诺当作真实能力。
- 重试优先顺序与有限突发恢复已有确定性测试源码，但持续异步编码饱和下的公平性尚未运行验证，不能声称已证明无饥饿。
- 本轮修复不等于全代码正确性证明；平台权限、复杂应用弹窗、桌面锁屏/热插拔、多设备长时间资源压力仍依赖原生验收。

## 本地集成与补验（2026-10-25，macOS）

- 在基准 `006615a96035497d9b899e2e1578bfa0b1850529` 上无冲突应用交付补丁，并按修复主题分别提交到本地 `main`。
- 原始交付文件为 `paneferry-review-fixes-2026-10-25.zip`，SHA-256 为 `d63f11034e40d3a39b65204e8757e361c8536014a12d9ba98e5f4b95d30720b1`。同时核对了内附完整源码：除两份历史性能 CSV 的 CRLF/LF 差异外，源码与应用补丁后的文件一致；保留本地 CSV 换行。Rust 改动已按项目格式化要求整理。
- 新增的 16 MiB 请求边界测试暴露了 `Wire.read()` 每收到一块数据就重新扫描整个缓冲区的问题。首次运行持续占用 CPU 数分钟后主动终止该测试，改为每次只扫描新增字节，并增加连续缓冲请求回归及编译/执行超时。补修后的真实 socket 边界回归通过，测试进程受 30 秒执行期限约束。

实际通过的检查：

- `scripts/check.sh` 前半段：根 Rust 工作区 106 项通过、10 项按环境/显式选择条件跳过；格式检查、Python/JavaScript 回归、GTK 管理器回归、前端 36 项测试及生产构建、Tauri 6 项测试均通过。
- 根工作区已执行新增的 QUIC 流额度耗尽超时、旧媒体流拒绝、两级原始帧队列压力、重采集顺序、空闲后反馈宽限、桌面会话回收，以及 Windows 请求绑定和缩放竞态的模拟后端测试。
- 首次完整脚本在上述慢请求测试处主动中止，未将该次运行记为整体通过。补修后单独运行 `python3 tests/e2e/mac_publisher_boundaries.py` 通过，再按原脚本顺序执行其余 macOS 门禁并全部通过，包括重新构建发布端、接收端，VideoToolbox、异步解码、实际 `SurfaceView` 缩小后释放输入、子窗口、剪贴板、自动重连、H.264 和本机双 QUIC 节点传输。
- `python3 tests/performance/scene_probe_unit.py` 通过。
- 集成差异的 `git diff --check` 通过。

剩余验证限制：

- Windows MSVC 目标检查未通过环境准备阶段：默认 Homebrew Rust 缺少该目标；显式使用已安装目标的 rustup 工具链后，`ring` 的 C 编译因缺少 Windows SDK/C 头文件 `assert.h` 受阻。未运行 Windows 原生发布、WGC/Media Foundation 或 Windows 实机缩放测试；Mac 上的模拟后端回归不替代它们。
- 未运行 Linux Xvfb/GStreamer 原生压力验证或新的物理双机验收；队列公平性的长期压力结果仍未确认。Clippy 未运行，远程 CI 未作为本轮验证依据。
- 本次执行只合入本地，未推送或部署。确认提交及验证记录保存后，按用户要求清理交付压缩包与本次解压、检查产生的临时文件。
