# 审查修复版部署与 IDEA 实测（2026-10-25）

已将 `6ca4383b84300b50c1d3e355165ad4de4d647dd0` 的 Ubuntu Agent、network 和 GTK 客户端部署到远端，同时构建并启动新版 Mac 启动器和 network。真实 IDEA 项目树右键菜单完成 2 次预热及 **60 次正式打开、绘制和关闭**，无失败样本：延迟中位数 **323.635 ms**，最近秩 P95 **387.659 ms**，最大 **500.849 ms**。

这些数字不能证明比旧版更快或更慢。[上一次测试](2026-10-25-idea-popup-latency.md)使用 Git 日志菜单，本次使用项目树菜单，主窗口与菜单尺寸均不同，没有同条件旧版基线。

## 部署核验

- 远端 `remoteapp-agent.service` 和 `paneferry-network.service` 已重启并保持 active，测试结束两者 `NRestarts=0`。启动前确认只有外部桌面会话；没有终止用户的 IDEA 或 Codex。测试后两者仍运行。
- Linux 二进制以 release 模式构建，替换前保存备份；GTK Python 文件同步到安装目录。私有回滚文件及安装清单位于远端 `~/.local/state/lan-remoteapp/upgrades/audit-20261025/`。
- 通过 `/proc/<pid>/exe` 核验正在运行的 Agent/network 摘要，不只检查磁盘文件。配对连接正常，能力包括 `native.h264`、`video.qos`、`video.keyframe`。
- GNOME 50.1 正在运行桥接版本 3；已安装扩展与本次源码摘要相同，GStreamer 为 1.28.2，无需再次注销桌面。
- Mac 使用 `apps/launcher/scripts/build-macos.sh` 完成构建和签名，启动新启动器及 network。正式 IDEA 缓存入口已启动，其实际 WindowHost 摘要与新构建一致。Mac 发布端 Node 未重启，不参与本次接收方向。

| 组件 | SHA-256 |
| --- | --- |
| Ubuntu Agent | `dbb4c4a548decfc7edd92a81899d10fe7de88213d09a842b5c1f48e2320885c7` |
| Ubuntu network | `6c9d873a0b0c6c95f0db9e36ec41cefb6f2bdd2d467f9be1089abab764cfcb5c` |
| GNOME extension.js | `9edc30c350094f70eccee15a8fd0840a7435970148e6e1ea4faa7e597a62f494` |
| Mac network | `d4b2c466136359b9d30b61ff9bdd10e3b740b772b7587a1e6c7050016faedf5c` |
| Mac WindowHost | `88e3f7674d991fa2e4a00cf6a61e90757c28a40f9554ccf29dd79c7f210aebbe` |
| Mac launcher | `ae61580e8f2d600ee068cf2e1793fe439d3f07c3ce76f9e8ff4f75e91ea3cd2c` |

Agent 启动日志有两行 `Authorization required, but no authorization protocol specified`。本次 Wayland 菜单路径正常，不能将本次通过扩展为所有 X11 授权路径均正常。

## 方法与显示检查

使用仓库既有 `tests/performance/live_end_to_end.py` 和 `PopupProbe.swift`，将相同产品 WindowHost 源码编译为临时计时副本；远端为正式部署版本。仅在观察确认的项目树位置发送右键和 Escape，不输入文本或执行菜单命令。

- 真实配对 QUIC 连接，Mac 到 Ubuntu `10.0.1.66` 的路由为 **utun4**，属于当前实际使用的隧道路由，不是直接局域网基准。
- 正常接入，主图像 2800×1936 像素，2× 密度；菜单 790×1868 像素。探针没有单独请求测试分辨率，正常接入本身会建立虚拟输出和 DPI。
- 在主图像源像素 `(430,200)` 触发右键，只接受输入后新出现、父窗口匹配、类型为 menu 且像素尺寸匹配的菜单绘制。2 次预热排除，60 次正式样本全部保留，没有忽略其他弹窗。
- 起点为 Mac `SurfaceView.rightMouseDown`，终点为匹配菜单的 `NSImage.draw` 返回；使用同一 Mac 单调时钟。全部样本是 H.264、VideoToolbox 硬件解码。

| 对象 | 源逻辑矩形 `(x,y,w,h)` | Mac 窗口矩形（点） |
| --- | --- | --- |
| 主窗口 | `(1968,48,1400,968)` | `(162,95,1146,824)` |
| 项目树右键菜单 | `(2183,141,395,934)` | `(338,46,324,765)` |

主图像内容区约 `(0.273,0,1145.455,792)`，源逻辑到 Mac 点的比例约 0.81818。菜单水平偏移 `215×0.81818`，宽高 `395×934×0.81818`，垂直位置按坐标方向转换后，与本地整数点几何一致。菜单自然延伸到主窗口下沿外，没有被裁进主画面。

已查看实际解码主画面与完整菜单图像；菜单文字从顶部到底部完整，60 次均为无边框、不成为键盘主窗口，Escape 均成功关闭。此项是**视频内容及真实 NSWindow 几何、样式核对**，不等于最终 WindowServer 桌面合成截图验收。

补充多级菜单观察时，正式 IDEA 接收窗口已经接管会话，探针收到 `session_controlled`，零样本；没有中断现有连接，也未计入性能数据。因此本轮不能声称嵌套菜单、工具提示、普通对话框全部验收。临时扩展的观察探针仅在本机编译，未保留源代码改动。测试结束正式 IDEA 接收窗口保持连接，且已核验为新版。

## 延迟结果

| 样本数 | 中位数 ms | P95 ms | 最小 ms | 最大 ms |
| ---: | ---: | ---: | ---: | ---: |
| 60 | **323.635** | **387.659** | 257.888 | **500.849** |

| 阶段 | 中位数 ms | P95 ms |
| --- | ---: | ---: |
| 输入函数至本地写出完成 | 0.189 | 1.302 |
| 写出完成至完整响应帧开始解析 | 278.761 | 358.580 |
| 协议解析、解码及图像准备 | 33.315 | 39.852 |
| 到主线程选择帧 | 6.520 | 10.651 |
| 选择帧至 AppKit 绘制返回 | 2.275 | 11.213 |

中位数采用中间两数平均，P95 采用最近秩；各阶段分位数不可直接相加。响应到达前耗时最多，但这个区间包含输入网络、IDEA 处理、采集等待、临时流/编码器建立、编码和返回网络，不能将它单独归因为网络或 IDEA。

本结果不包含鼠标硬件扫描、macOS 最终合成、显示器扫描及面板响应，也不代表打字、滚动等其他操作的延迟。

结构化统计见 [JSON](2026-10-25-audit-deployment-idea-latency.json)，包含预热及被拒绝观察会话的数值记录见 [原始记录](2026-10-25-audit-deployment-idea-latency-raw.json.gz)。会话标识已移除，不提交图像、项目文本或凭据。部署构建和真实菜单检查均完成；本轮没有产品代码修改，没有重复运行此前已通过的全量门禁，也不据此宣称 Windows 或 hosted CI 通过。
