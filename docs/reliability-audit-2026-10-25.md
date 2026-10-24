# 2026-10-25 深度审查整改

来源为用户指定的“代码深度审查”，基准 `3c62ed3`；本轮从 `9204fae` 开始。完整审查已读取。以下跟踪具体缺陷；WebRTC/Xpra 替换、混合画质和全量重写属于审查列出的后续方案评估，不作为已证明缺陷的修复前提。生产桌面不用于故障注入。

| 项目 | 状态与证据 |
| --- | --- |
| Windows 临时目录错误、Tauri 门禁遗漏 | `368807f`；读取实际 Windows job 日志确认 `/tmp` 不存在；本机网络测试与 Tauri 5 个测试通过。Windows runner 尚未重新执行 |
| 预认证大包、管理名额共享、包体无限等待 | `2cb4863`；8 KiB 独立握手、独立 admission、首字节后绝对期限；11 个网络单元测试通过，包括饱和时本机查询 |
| X11 无父窗口扩大范围、retired 终身限制 | `8185894`；明确 transient + 进程身份；512 次窗口生命周期单元测试；远端独立 Xvfb/H.264 的实际跨进程预览、无父窗口拒绝、重连和输入回归通过 |
| 网络媒体回收、慢窗口隔离、控制/媒体/设备预算 | 已实现非阻塞窗口队列、销毁取消、独立控制/媒体/设备配额、三平台 `video.keyframe` 静止画面恢复；网络组件/集成、Windows 模拟发布端、协议测试通过，Ubuntu Agent 28 测试通过（7 个环境测试忽略），Mac 构建通过；生产性能尚未验收 |
| GTK / Mac 接收端 retired 与窗口代次 | `8cc1dde`、`5b02207`；有序 destroyed/created 界定生命周期；GTK 130 次同 ID 重建、AppKit 原生窗口重用和迟到帧检查通过 |
| Linux 全局锁慢操作 | `b9c7b6d`、`f07d8d0`；启动串行锁与短期目录锁分离，发现使用快照；合成慢操作期间查询/停止共享可获得目录锁，Ubuntu 测试通过 |
| Mac 发布端异步写出与取消 | `b158a4c`；有界后台 writer、三秒绝对写期限；不读 socket 的真实夹具仍可立即发控制/finish，测试通过 |
| Mac 剪贴板输入闸门、窗口销毁取消 | `4a8928a`；只保留同目标 copy→paste 顺序，不积压普通输入；目标销毁取消 helper；实际 AppDelegate 私有剪贴板测试通过 |
| Mac→GTK 编码流/几何版本一致性 | `8594fc4`；相同 configure 幂等；版本改变重建编码流；真实 VideoToolbox 639→640、相同补齐尺寸仍返回新 IDR/stream 测试通过 |
| GTK 静止连接超时 | `6a5f1b6`；真实 socket 静止 16 秒仍能读取，另测请求和部分包体期限 |
| GTK 迟到复制覆盖本地剪贴板 | `abcec14`；监听系统 selection owner-change，使迟到回包失效；边界测试与 Ubuntu 实际剪贴板回归通过 |
| GTK 终结启动错误 request ID | `abcec14`；明确终结码清除，超时/未知失败保留；Python 与 Rust 终结码集合一致性检查通过 |
| Mac 双击/三击、down/up 序列 | `9c32665`；同窗口/按钮/位置/时间序列计数；真实 NSEvent clickCount、配对编号、间距复位测试通过 |
| GTK SPS 编码尺寸/DPB/参数集校验 | `8cc1dde`；限制 coded dimensions、参考帧预算与参数集变化，流首包必须 SPS/PPS/IDR；恶意大 coded、小 crop 测试不进入解码器；真实 GTK 解码通过 |
| Windows attach 清理竞态 | `459a025`；清理和成功接管均校验当前 controller；已退休/所有者更换的确定性测试与模拟发布端通过；原生 Windows 桌面尚未运行 |
| GTK popup/tooltip 呈现 | `f2d4294`、`21ec22e`；无边框、源相对位置/尺寸、保持父焦点、禁止 popup configure；Ubuntu 实际 GTK 菜单和 130 次生命周期通过 |
| 配对丢响应后的幂等恢复 | `1d3634d`；持久化同证书邀请 claim，在有效期内重试；第二身份拒绝，撤销清除 claim；组件及三设备 QUIC 配对回归通过 |
| 停止共享取消在途 bulk | `1d3634d`；250ms 轮询、单次一秒期限；禁用/失联取消传输与子进程，不发成功终结符；撤销监视与截断测试通过 |
| Mac 发布端剪贴板缓存累计回收 | `e95310c`；保留当前引用，32MiB / 128 目录 / 七天回收；临时目录中的引用保护与过期回收测试通过 |
| WindowHost 缓存代次回收 | `ab3a218`；保留当前、运行中及一天内新建代次，旧代次按七天/保留数量收集；Tauri 实际文件系统测试通过 |
| Mac 编码错误传播 | `8594fc4`；检查属性设置、prepare、EncodeFrame、CompleteFrames 和回调；失败进入 capture_failed；构建与真实编码回归通过，未故障注入 macOS 驱动 |
| 绘制反馈时机与协议状态语义 | `5b02207`；AppKit/GTK 都在 draw 回调后反馈；实际 AppKit cacheDisplay 验证排队时无反馈、绘制后才有；文档明确不包含最终合成/扫描 |
| 三平台分层 CI 与故障交错测试 | `b12c9f2`；保留 Windows gate，新增 macOS / Ubuntu workflows，常规门禁覆盖 Tauri、Wire、媒体边界、VideoToolbox、GTK/AppKit；GitHub hosted workflows 尚未触发 |

本轮所有性能数字需标注测量终点；构建、组件测试、原生桌面验收和 CI 分开记录。逐项修复和提交，不将原有门禁通过视为这些新反例已经被覆盖。

补充（`3700077`）：Mac 发布端同步收紧关联规则，辅助功能显式父链与进程身份必须同时成立；不再用最近输入推测父窗口。缺少 AX 父链的独立辅助窗口仍可单独选择共享。此规则的构建和边界测试通过，尚未进行用户真实 macOS 多应用弹窗权限验收。

额外故障回归：真实 QUIC 连接中停止消费窗口 1（超过其流控窗口），仍在期限内收到窗口 2 的画面和控制消息；恢复消费后静止窗口主动取得 IDR，取消后每设备媒体额度全部归还。另测 512 次网络元数据创建/销毁不积累活跃配额。

本轮不以功能修复宣称延迟改善；没有同条件生产基线，也没有最终屏幕合成/扫描的测量。验证运行在本机测试进程和 Ubuntu `/tmp/paneferry-audit-20261025` 隔离源码环境，未重启用户正在使用的正式 Agent、IDEA 或 Codex。

## 最终执行结果

- 本机 `./scripts/check.sh` 退出 0：该次 Rust 测试合计 99 passed / 10 ignored（含独立 Tauri workspace 的 6 项），前端测试/构建、Swift、AppKit、H.264、重连、真实 QUIC 与二进制剪贴板回归均通过。随后新增的慢媒体故障测试单独执行，并复跑网络包，22 passed / 0 ignored。
- Ubuntu 隔离源码 `cargo test --workspace`、Agent/network 构建、实际 GTK/GStreamer 接收、X11 应用发现回归均通过；额外显式运行 native_encoder 捕获租约/像素回归通过。最后的 Linux 锁清理及网络故障用例增量回归合计 50 passed / 7 ignored。
- `cargo fmt`（根工作区及 Tauri）、Python 语法、`git diff --check` 通过。没有跳过提交钩子。
- ignored 表示测试未在该轮默认门禁执行，不能算通过；Ubuntu native_encoder 已另外显式执行。Windows 原生编译/桌面、GitHub hosted CI、正式部署和生产端到端性能仍未验收。

日志保留于本机 `/tmp/paneferry-audit-full-local.log`、`/tmp/paneferry-audit-full-ubuntu.log`、`/tmp/paneferry-audit-network-final.log`、`/tmp/paneferry-audit-final-ubuntu-delta.log`，临时目录日志不作为仓库长期证据；上面的命令和用例已纳入版本控制，可重复执行。

后续已按用户要求部署 `6ca4383b` 到 Ubuntu 并更新 Mac 接收端，完成真实 IDEA 菜单 60 次端到端测量；部署摘要、显示证据、延迟及验收范围见[部署实测记录](performance/2026-10-25-audit-deployment-idea-latency.md)。该后续结果不改变上述审查阶段的历史执行范围。
