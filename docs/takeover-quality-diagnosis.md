# 已有 ChatGPT 窗口接管清晰度定位（2026-10-24）

**已定位源密度不足及编码损失；未实现同尺寸高清修复。** 当前目标为已有 XWayland ChatGPT 进程，不是原生 Wayland 窗口，也不是 Agent 新建的 Xvfb 实例。本轮只增加显式执行的一次性诊断程序和隔离对照测试，没有修改产品的启动、捕获、编码、显示、输入或协议行为。

初始 HEAD 为 `3284dad91c16bd19c7a490875aa04b81110e6c99`，工作区干净。没有清空、回滚或覆盖先前改动。之前已提交的私有 X11 DPI、Wayland 输出及生命周期修复全部保留。

## 实际调用链

`main.rs::launch()` 的分支顺序是复用已有会话、发现桌面窗口、最后新建进程。不能以 `app.launch`、应用卡片或 `session.resume` 判断进程来源。

| 实际来源 | 源密度和捕获 | 与其他路径共用的部分 |
| --- | --- | --- |
| 接管原生 Wayland | `wayland::Backend::connect` → helper `acquire` → 扩展 `ensureOutput` → `VirtualOutput.open`；移入私有输出后才 RecordWindow/PipeWire，按 crop 和窗口边界提取原像素 | `WindowBackend`、Output、OpenH264、QUIC、Mac 解码与 SurfaceView |
| 接管 XWayland/X11 | `Backend::connect_existing`；按原 PID/启动时间及 XID 限定范围；XComposite pixmap/SHM 或 GetImage；XWayland 可用桥处理几何和输入，但 `ensureOutput` 只对原生 Wayland 创建输出 | 同上；`set_scale_factor()` 只赋值元数据，不改变应用 DPI |
| 新进程 | `Session.existing == None`，Agent 拥有 child；创建私有 Xvfb、DPI/Xft 资源、启动环境，再启动应用 | 同上；重连该会话仍属新建实例来源，不变成桌面接管 |

发现的 X11 桌面窗口由 `discovery::RunningWindow::app()` 填入 `existing.scale_factor=1`，这不是工具包 DPI 测量。此次目标的实际 GNOME 资源倍率也为 1，不能改成 2 来伪造密度。

`window.configure.width/height` 是源像素。Mac `HostedWindow.sendResize` 使用内容 points × **源倍率**；Wayland 后端除以实际 `Info.scale` 后请求合成器逻辑尺寸，X11 请求客户区像素。`SurfaceView.imageRect` 按实际解码图像等比拟合；指针按图像矩形映射到源像素。此轮未发现需要再乘一次 Mac backingScaleFactor 的证据。

原生 Wayland 的 `video.rs` 校验 PipeWire crop 与预期 buffer 尺寸一致，拒绝错配/越界；X11 捕获不包含缩放器。编码器做 BGRx→I420 和奇数宽高补边，未加入 videoscale；Mac 解码校验编码尺寸并裁去补边。真实流接收端按 geometry_revision 和实际尺寸筛选展示帧；本次只读采样不能读取正在运行会话的 revision，不伪造其值。

## 当前运行版本与目标

通过已有配对连接仅执行 hello/window.list，通过获准 SSH 读取进程、X11 和 GNOME 元数据。未另行接管、聚焦、resize 或输入真实窗口。

- ChatGPT PID **14785**，内核启动计数 **5129**，XID **52428804**，display **:0**，原会话 **d6f3f8007b0d375c1e55396fb1d535e5**。
- `XWAYLAND` 扩展存在；GNOME 同一 PID/XID 返回 `native=false`、`scale=1`，客户区、frame、buffer 都是 **1470×809**；X server 为 96 DPI。96 DPI 和资源倍率仅描述已测环境，未据此推断应用所有内部缩放设置。
- Agent PID **1619731**，GNOME Shell PID **6098**；`paneferry@local` 为 ACTIVE，活动桥返回版本 **2**。
- 安装的 `virtualOutput.js`、`textInput.js` 和 metadata 与仓库逐字节相同；`extension.js` 缺少后续全桌面剪贴板分支，窗口和虚拟输出代码一致。GNOME 会缓存模块，文件相同不等于已证明内存中全部 JS 相同。
- 正在运行路径中的 Mac WindowHost 与当前打包副本 SHA 相同。Agent 和 Mac 程序没有可读取的 commit 构建标记，**运行二进制是否精确来自 HEAD 仍未知**；哈希不能替代源版本证明。

全部校验值、尺寸和量化数据见 [结构化记录](validation-artifacts/2026-10-24-takeover-quality.json)。没有将旧验证文档的“停用扩展”状态当作当前事实。

## A/B/C 证据

| 阶段 | 实测或重建结果 | 边界 |
| --- | --- | --- |
| A 原始源图 | 对准确 XID/PID 只读命名 XComposite pixmap，获取 **1470×809** 原图；采样前后客户区及进程启动计数一致 | 一次快照，不持续记录；没有应用重绘或 resize |
| B 同源编码/解码 | 同一快照通过当前源码 Encoder 和 Ubuntu OpenH264，编码 **1470×810**；Mac 生产 VideoToolbox 解码器裁回 **1470×809**，硬件解码；每组一个 IDR、两个依赖帧 | 独立编码，未截获生产流；分别测 60/90 cap，生产实时 cap 未知 |
| C 实际 Mac | 活动宿主显示“接收 1470×809 像素 / 本机 1470×809 点 / 放大 200%”；只读截图观察到文字发软 | 此截图与 A 的采样不是同一时刻，不能当作像素差分对 |
| C 同帧绘制重建 | 复用生产 SurfaceView，imageRect 为 **(0,0,1470,809) points**，2× backing 对应 **2940×1618**；宽高放大均 **2.0** | 离屏绘制，不等于物理面板截图或端到端时延 |

计算使用 imageRect，未包含标题栏。源原图在 1:1 像素下能辨认文字；解码对照有额外边缘损失，没有发现额外缩小尺寸或错误裁切。

同源三帧 RGB 平均绝对误差（0–255）在 60 cap 下为 **1.504 / 1.444 / 1.398**，原图边缘区域为 **8.272 / 7.853 / 7.483**；90 cap 下分别为 **1.369 / 1.352 / 1.349** 和 **7.331 / 7.178 / 7.172**。边缘掩码以原图相邻像素任一 RGB 通道差大于 24 定义。指标不是文字可读性评分，也不能给“编码占模糊百分比”下结论。

**已证实 1× 源被放大、同时存在有损编码损失；未证实编码是主要损失。** 即便消除全部编码损失，1470×809 原图仍不能提供 2940×1618 的原生细节。因此未调高码率、降低 QP、切换编解码器或全局调整共享渲染器。

## 同内容的两条路径对照

在临时 HOME、独立 Xvfb/Openbox 和独立 Agent 中运行相同 GTK 文本夹具。已有组先创建进程，再以包含准确 PID 的 existing 配置接管；新建组由 Agent 实际创建另一个私有 Xvfb 和进程。两组都发送 `app.launch(scale_factor=2)`，测试依据 PID、display、会话状态判断真实路径。

| 指标 | 已有进程接管 | 直接新启动 |
| --- | --- | --- |
| GTK 实测倍率 | 1 | 2 |
| GTK 逻辑尺寸 | 900×700 | 900×700 |
| Pango 逻辑字体高度 | 19 | 19 |
| Agent 实际视频像素 | 900×700 | 1800×1400 |
| Mac 同为 900×700 points、2× backing | 放大 2 倍 | 1:1 |
| scale_mode | source | requested |
| 进程 | 原 PID/内容/布局保持 | 确认为另一个新 PID、私有 display |

两组窗口元数据和真实 H.264 帧的 geometry_revision 一致。此对照证明现有策略能产生用户观察到的差别，且新启动的字体大小没有变小；它不证明已有 ChatGPT 能在不重启时变成 2×。

## 本次改动与复现

产品源文件没有改动。新增 `quality_sample` Cargo example、Mac 离线解码/绘制工具和隔离对照脚本；Cargo 只增加 example 使用的 dev-dependency，无新增版本依赖。

Ubuntu 显式执行一次采样（目录必须不存在，DISPLAY/XAUTHORITY 必须是已核实的目标 X server）：

```sh
cargo build --locked -p paneferry-agent --example quality_sample
DISPLAY=:已核实的编号 XAUTHORITY=/已核实的授权文件 \
  target/debug/examples/quality_sample 十进制XID 原进程PID /tmp/新的采样目录
```

探针使用 XRes 验证 PID，检查进程 starttime 和采样前后尺寸，拒绝隐藏窗口及超限尺寸；不调用 Backend::scan、窗口重定向、resize、focus、租约或输入。每次只读取一张原图，对这张原图执行两组固定三帧编码，不接入正常传输循环。无启用开关、后台监听或自动日志；不运行 example 就没有采样。

导出目录权限 0700；包含用户画面时应保留在私有目录，不加入 Git。此次真实画面只留在忽略的 `dist/takeover-diagnosis-20261024/chatgpt-private/`，仓库仅存尺寸、身份、哈希和汇总误差。没有账号内容进入已提交证据。

将采样目录复制到 Mac 后，按实测内容区和 backing density 重建：

```sh
python3 tests/performance/quality_sample_mac.py /本地采样目录 \
  --content-points 1470 809 --backing 2
```

输出 `mac-analysis/report.json`、解码 PNG、同帧绘制 PNG。它不访问网络或控制远端应用，拒绝覆盖已有分析目录。参数来自实测内容区；不能填写包含标题栏的外框尺寸。

离屏工具自身的方向、1×/2× backing 像素一致性和留边位置检查：

```sh
python3 tests/performance/quality_sample_mac.py --self-test
```

该检查用不对称色块先发现并纠正了新增诊断工具的离屏坐标翻转；生产 SurfaceView 没有这个新增工具中的错误，也没有被修改。证据中的绘制结果使用纠正后的工具重新生成。

完整隔离对照（仅生成夹具文本）：

```sh
cargo build --locked -p paneferry-agent --bin paneferry-agent --example quality_sample
python3 tests/e2e/takeover_quality_smoke.py \
  --agent target/debug/paneferry-agent \
  --probe target/debug/examples/quality_sample \
  --output /tmp/新的对照目录
```

本次未新增生产端诊断协议，也未更改任何已有协议字段含义。`sample.json` 中 `geometry_revision` 和 `source_scale` 为 null，明确表示只读探针无法从生产流取得该值；隔离测试另外保存真实 Agent 的 frame 和 revision，不能将两者混用。

## 验证与未验证范围

- 修改前完整 `bash scripts/check.sh` 通过：Rust **59 passed / 6 ignored**、前端 **32 passed**，Swift/AppKit、键盘释放、点数/像素换算、连续 resize、旧图像过渡、留边输入、拖拽释放、关闭断开、重连及真实本机 QUIC/H.264 通过。
- 修改后完整 `bash scripts/check.sh` 再次退出 0；Rust 和前端计数相同。完整日志保留在本地忽略目录 `dist/takeover-diagnosis-20261024/`，不将本地门禁称为 CI 或生产验收。
- Ubuntu 私有仓库副本完成 workspace 测试、Agent/example 构建；显式 Agent Linux 编码测试 **25 passed / 0 ignored**，含实际 GStreamer/OpenH264、PipeWire 就绪唤醒及源像素不变检查。
- 新隔离对照和采样拒绝错误 PID/覆盖目录检查通过；既有 `hidpi_smoke.py` 通过，GTK 字体高度 19/19、Xft 终端像素随私有 DPI 改变。
- Mac 对真实窗口单帧和两组夹具的解码/离屏显示均执行；奇数高度正确补边/裁回，源 1×/2× 的 imageRect 放大比分别为 2/1。
- 新增离屏方向/密度/留边检查先失败、修正工具后通过；已有分析目录覆盖和非有限视口参数均被拒绝，原报告保持。未对产品渲染代码做修正。
- `cargo clippy --locked -p paneferry-agent --example quality_sample -- -D warnings`、Python 语法和 diff whitespace 检查通过。全项目修改后门禁记录见 [validation.md](validation.md)。

未对真实 ChatGPT 发送中心/四角点击、滚动、快捷键、resize、断线重连、菜单或 transient 操作。上述基线交互通过是本地夹具结果，不能称为当前真实应用的交互验收；没有改动这些路径。没有新虚拟输出生命周期变更，未在用户桌面或隔离 GNOME 重做危险关闭竞态。没有生产画质改善、CI、持续 CPU/带宽/FPS/延迟或多屏验收结论。

结束时再次核验原进程启动计数和服务状态一致；确认临时测试程序已退出后，清理本次独占的远端 `/tmp/pane-ferry-takeover-20261024`（含临时画面副本）并关闭本次 SSH 主连接。没有操作 VPN 或用户桌面生命周期。

## 结论与下一步边界

当前目标走 XWayland；原生 Wayland 的现有高清输出链路不能直接让这个已运行进程提交高密度缓冲区。项目中没有经本次验证、在不重启该进程、不改全局桌面、不注入进程、不自动 zoom 的前提下实现同尺寸 2× 重绘的方法。这里是本次受约束路径的能力边界，不是对所有 X11 应用能力的普遍断言。

原始像素显示可用于对照，但会让窗口/控件变小；未将其当作修复，也未替用户切换。后续若继续探索，应先在同版本 ChatGPT 的独立配置/隔离桌面验证工具包真实重绘、字号和输入，再讨论是否改变当前边界。此次不以实验方案替换正常启动路径。

撤销本次改动只需依次 `git revert` 本次证据记录提交和诊断工具提交（具体 SHA 见交付消息）。没有部署或运行服务变更需要回滚；不可回滚之前的 DPI、输入或生命周期修复来撤销本轮诊断。
