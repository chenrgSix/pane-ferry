# QUIC 独立双向链路与 SSH 移除验收

2026-10-22：先验证 QUIC 可以独立完成两个方向，再移除产品中的 SSH，最后重新执行双机回归。实现提交为 `de1afb0`，真实 AppKit 夹具为 `0cf7df3`，Linux 旧连接参数校验修复为 `00d0248`。每个平台仍交付一个 LAN RemoteApp，同时包含发布和接收能力。

## 测试条件

| 项目 | 本次条件 |
| --- | --- |
| Mac | macOS 27.0.1、arm64、Apple M4；真实 AppKit 接收器，已获用户授权的 Mac 发布组件 |
| Ubuntu | Ubuntu 26.04.1、x86_64、Intel i5-13400；独立测试 Agent、Xvfb/GTK/GStreamer |
| 网络 | EasyTier 虚拟地址 `10.0.1.66`；沿用已有网络配置，未调整代理、拥塞控制或窗口参数 |
| 身份与会话 | 任务私有状态目录、临时证书配对、专用 GTK 编辑器与 MacPublishingFixture；未操作用户已有应用内容 |
| SSH 的作用 | 仅开发部署、读取测试结果和清理。删除后的双机业务测试开始前，任务专用 SSH master 已关闭；业务进程通过 QUIC 配对运行 |
| 发布器版本 | Ubuntu 使用新 Agent；Mac 发布器没有代码变化，与新包内发布器 SHA-256 相同 |

本次是功能与依赖移除验收，不是性能实验。未重复三轮吞吐测量，不把首帧耗时、短时并行或移除代码解释为性能提升。历史性能结果保留在 [SSH/QUIC 对照](performance/2026-10-22-ssh-quic.md)；对应历史夹具须在 `c927116` 检出中复现。

## 删除前后结果

| 验证项 | 删除 SSH 前 | 删除 SSH 后 |
| --- | --- | --- |
| Ubuntu → Mac | QUIC 上的真实 xterm：键鼠、缩放、隐藏恢复、启动幂等、单控制端、3 次重连通过 | 真实 GTK 编辑器 → QUIC → AppKit：H.264 解码、中文/emoji、文本复制/粘贴/剪切、960×640 缩放、关闭远端应用；独立与并行场景各通过一轮，每轮 3 次同窗口/同会话自动重连 |
| Mac → Ubuntu | 真实 Mac 窗口 → QUIC → GTK：串行复测通过 | ScreenCaptureKit/VideoToolbox → QUIC → GTK/GStreamer：点击恰好一次、中文/emoji、双向文本剪贴板、剪切后删除及按键释放、1200×840 缩放、原会话重连和新解码帧、断开保留 Mac 应用均通过 |
| 双向并行 | 双向基础能力已确认 | Ubuntu 连续接收 Mac H.264 60 秒，同时 Mac 完成正向全套交互和 3 次重连；反向连接代次未变、保持 ready，随后自身重连通过 |
| 多设备 | 既有网络层配对/撤销测试 | 三个本机真实 QUIC 节点：A→B、A→C、B→A；撤销 B 后 B 桥接结束、C 仍可用；旧返回路由记录和裸设备 UUID 不被当作可连接入口 |
| 大剪贴板 | 既有隔离传输测试 | AppKit 通过两个真实本机 QUIC 节点往返超过 1 MiB PNG、目录、Unicode 文件名、空文件；Linux GTK 文本/图片/文件隔离回归通过 |

首次删除前的反向 Unicode 断言，在另一个 AppKit 测试并行运行时失败；串行重测及删除后的测试均通过。没有证据把该次失败归因于 QUIC 协议。双向并行测试刻意只让一个方向发送输入和使用剪贴板，不代表同时争用两端焦点/系统剪贴板已经验收。

## 自动检查

| 检查 | 结果 |
| --- | --- |
| Mac `bash scripts/check.sh` | 完整通过；Rust 44 项通过、4 项按条件忽略；Python 授权单测、前端 22 项、类型检查/构建、Swift 编译与 AppKit 回归通过 |
| Tauri 后端 | 5 项通过：仅配对 UUID 可作为连接目标、旧密码配置清理及符号链接保护、应用宿主身份隔离 |
| Rust 格式、工作区和 Tauri Clippy | Mac 通过，Clippy 使用 `-D warnings` |
| Ubuntu 适用检查 | Rust 44 项通过、4 项忽略；Python、三节点路由、实际 GTK/QUIC 和 release 构建通过 |
| Linux 最终修复 | 非法或历史 SSH 连接参数正常返回失败；实际 GTK/QUIC 回归及重新打包通过 |
| 包内检查 | Mac launcher、WindowHost、Node、network，以及 Linux 关键发布/接收组件未含 `/usr/bin/ssh` 调用；源码只保留旧配置清理和拒绝历史路由的回归 |

Ubuntu 缺少 rustfmt 和 Node/npm，完整 `scripts/check.sh` 因缺少 rustfmt 停止；没有将其记为完整通过，也未安装新工具。上述 Ubuntu 检查单独运行，格式与前端门禁在 Mac 完成。Mac AppKit 需要正常图形会话，受限沙箱中的未完成尝试不计为通过。

结构化结果、产物大小及 SHA-256 见 [JSON 记录](validation-artifacts/2026-10-22-quic-only.json)。[压缩日志](validation-artifacts/2026-10-22-quic-only-logs.json.gz) 保存两端门禁、反向及并行验收、Clippy、Tauri 和打包输出；正向 AppKit 的两轮 PASS 来自执行器输出，未另存完整逐行日志。日志不包含邀请或私钥。

## 复现入口

在仓库根目录运行 `bash scripts/check.sh`；实际 Linux GTK 测试为 `xvfb-run -a env GTK_IM_MODULE=simple GDK_BACKEND=x11 python3 tests/e2e/linux_receiver_smoke.py`。图形依赖、编译器和网络组件须已安装或构建。

双机测试需要先在隔离目录启动发布/网络组件并完成临时配对。Linux 测试 Agent 的自定义应用必须指向 `tests/e2e/clipboard_editor_fixture.py`；Mac 使用 `tests/fixtures/MacPublishingFixture.swift` 中的专用测试应用，并沿用用户已开启的共享和系统权限。准备后关闭本次部署使用的 SSH master，再运行：

```sh
# Mac：STATE 为含 control.sock 和 peers.json 的网络状态目录。
python3 tests/e2e/mac_receiver_live.py \
  --peer UBUNTU_DEVICE_UUID --state STATE \
  --network /absolute/path/to/remoteapp-network --test-app quic-editor

# Ubuntu：ROOT 下包含 network/；不是直接传入 network/ 目录。
xvfb-run -a env GTK_IM_MODULE=simple GDK_BACKEND=x11 \
  python3 tests/e2e/mac_to_linux_live.py \
  --peer quic:MAC_DEVICE_UUID --state ROOT

# 反向仅保持画面，此时可并行运行上面的正向 AppKit 测试。
xvfb-run -a env GTK_IM_MODULE=simple GDK_BACKEND=x11 \
  python3 tests/e2e/mac_to_linux_live.py \
  --peer quic:MAC_DEVICE_UUID --state ROOT --capture-only --hold-seconds 60
```

`native_session_smoke.py` 还会通过开发 SSH 读取测试标记和 X11 状态，适合补充诊断；它不能单独证明测试全程无 SSH。上述新增 `mac_receiver_live.py` 不部署、不配对、不调用 SSH，以真实 AppKit 回读验证交互。

## 交付与限制

开发包已构建到 `dist/lan-remoteapp-quic-only-macos/LAN RemoteApp.app` 和 `dist/lan-remoteapp-linux-quic-only.tar.gz`。没有覆盖正在运行的旧安装：Ubuntu 原 Agent 仍有一个用户会话，升级安装会重启 Agent，故本次使用独立测试实例；Mac 新 launcher 也未替换原运行实例。新版首次运行会清理本应用的 SSH 密码文件和旧界面配置，保留配对 QUIC 身份、目录缓存及未确认启动请求，不修改系统 SSH。

三节点测试不是三台物理设备。尚未完成三台以上实机并发、物理断网/网络切换、休眠唤醒、长期负载、两个方向同时输入、Ubuntu 物理输入法候选选择、图片/文件的本轮双机原生应用验收，也未执行 CI、发布签名、公证或生产验收。本轮保留原有共享权限、EasyTier 配置和已安装服务。

验收结束后，已停止临时发布器、网络进程和测试应用，关闭任务专用 SSH master，并删除两端任务临时目录及临时配对身份；仅保留开发包和上述验收证据。原 Ubuntu Agent 仍为 active，未重启。
