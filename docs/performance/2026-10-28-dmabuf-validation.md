# 私有 DMA-buf 直达 VA 转换/编码的本机验证

**本机原型验证通过，值得继续接入 Agent。** 2940×1618 滚动夹具中，采集提交到 H.264 码流可取的
P50 从软编的 **21.29 ms 降到 13.12 ms（−38.3%）**；先读回再交给 VA 的路径为 30.16 ms。
这是独立、串行原型的结果，**尚未接入或部署正式 Agent，不能视为采集到发布的实际提速**。

[统计与检查结果](2026-10-28-dmabuf-validation.json)、
[全部预检、失败记录、逐帧时间、跟踪及源码快照](2026-10-28-dmabuf-validation.raw.json.gz)。
它接续[上轮 GPU 读回/上传诊断](2026-10-28-gpu-readback-upload.md)。

## 范围与方法

Ubuntu 26.04.1、GNOME 50.1 / Wayland、XWayland、i5-13400 / UHD 730、内核 7.0.0-38、GStreamer 1.28.2。
Intel VA 驱动 26.1.2 及开发头文件只解包在临时目录，运行时环境仅作用于测试进程，没有安装系统包。

在当前 XWayland 中新建私有 source pixmap 和三个私有 GBM 目标，不创建可见窗口，不读取用户 drawable。
初始测试图仅上传一次，随后 GPU 按 11 像素步进滚动，顶部显示每帧二进制计数。
三目标轮换；每次等待当前编码结果后再开始下一帧，直到管线 NULL 后才销毁目标。
因此验证的是串行复用，尚未验证异步多帧在途的租约池。

对照路径：

1. 私有目标 → CPU 不可变 BGRx 快照 → videoconvert/I420 → x264。
2. 同样快照 → vapostproc → GPU NV12 → vah264lpenc。
3. 私有目标 FD → GstDmaBufMemory + GstVideoMeta → vapostproc → GPU NV12 → vah264lpenc。

CPU 路径持久只读映射，执行 DMA-buf poll、READ START/END 和一次逐行快照复制。
直接 GPU 路径不创建 CPU 输入映射，复制 FD 的所有权交给 GstMemory，保留实际 stride；仍等待渲染完成。
GStreamer 所有缓冲引用与私有 BO 的生存期由测试进程保留至编码器关闭。

三路均保留 2940×1618 输出、60 FPS 的 PTS/DURATION、28.54 Mbps 名义码率、QP 上限 28、GOP 60、无 B 帧。
软编沿用当前产品的 ultrafast/zerolatency、4 个编码线程和按几何选择的 3 个转换线程；两条 VA 路径参数相同。
不按 60 FPS 节流，以测串行服务时间；原始吞吐不代表正式会话帧率。

按 `软编 / 快照 VA / 直接 VA`、反序、正序、反序串行运行，共 12 轮。
每轮 16 帧预热、90 帧统计；没有并行编译或运行其他基准，用户其他任务保持运行。
先记录时间，之后才解码、核对计数和 GOP。CPU 数为测试进程所有线程，不包含 XWayland、合成器或 GPU。

## 两个必须保留的条件

### 存储行对齐

本机 GBM 为 2940 像素线性目标给出 11760 字节 pitch。该缓冲虽被 VA PRIME2 导入成功，解码却出现斜行错位，
全图 MSE 约 3091，补上 DMA-buf poll 或重新导出 FD 都不能修正。这是像素布局错误，不能仅按帧计数异常判断为旧帧。

将 GBM **存储宽度**对齐到 16 像素，获得 11776 字节 pitch，DRI3 pixmap 和输出仍为 2940×1618，
GstVideoMeta 传递真实 stride 后通过校验。没有缩小、拉伸或裁掉可见像素。
1470×808 也使用相同方式分离可见尺寸与存储 stride。
这只是本机路径验证出的兼容条件，不能推断所有 VA 驱动都需要或支持同一对齐。

### 显式 XSync fence 等待

只有 TriggerFence、XSync 往返、DRI3 重新导出和 DMA-buf poll 时，CPU 对照偶尔读到目标上一次使用的内容，
例如输入/输出计数均为 55，而当前应为 58。增加编码前的 CPU 快照标记检查后，确认异常已经存在于输入，
不是 H.264 解码产生。所有无 AwaitFence 的轮次均排除出最终性能比较，原始结果仍归档。

在三路统一加上 XSyncAwaitFence 后，5 轮专门软编复核的 530 帧，以及最终比较、复用、码流和跟踪的
2914 帧全部计数正确。尚未在正式 Agent 中复现这个偶发问题或验证修复，不将其描述成已发布的产品修复。
[XSync 协议](https://www.x.org/releases/current/doc/xextproto/sync.pdf)区分触发排队与等待 fence；
本次结果也不能证明 fd poll 足以替代所有生产者同步。

## 最终性能对照

每路汇总 4 轮、360 个预热后样本，不拼加不同阶段的分位数：

| 路径 | 采集提交 → 码流 P50 / P95 | 进程 CPU 时间 / 帧 |
| --- | ---: | ---: |
| CPU 快照 + x264 | 21.29 / 31.60 ms | 35.13 ms |
| CPU 快照 + GPU 转换/编码 | 30.16 / 39.69 ms | 16.03 ms |
| DMA-buf 直接 GPU 转换/编码 | **13.12 / 18.17 ms** | **1.38 ms** |

直接路径相对软编的各轮 P50 分别减少 **40.4%、36.5%、39.2%、35.0%**，方向一致。
相对同参数快照 VA 路径，各轮减少 52.8%–58.6%。GPU 颜色转换和编码仍约 8.67 ms，
输入准备仍包含约 3.35 ms 的 fence 等待；不能把直接路径叫作无等待或无 GPU 复制。
以上数字包含夹具的 X11 往返及编码输入包装，不包含 Agent 队列、去重、JSON/socket 发布、QUIC 和远端呈现。

## 画质、复用与导入核对

- 640×480、1470×808、1920×1080、2940×1618，各 3 帧滚动图全帧解码检查通过。
  每帧直接 GPU 路径相对同帧 CPU 参考的 MSE 均低于软编；2940 尺寸约 70.1–107.6，对照约 85.8–118.0。
  这只是小型测试图检查，不代表所有应用、颜色空间和细字内容的画质保证。
- 三路各额外 316 帧轮换三个缓冲，覆盖计数 255→0 和 6 个 GOP，全部逐帧正确。
- 快照 VA 和直接 VA 又各编码 316 帧，**完整 H.264 流逐字节相同**：
  SHA-256 `26c4a81afbc1c50455fdd1cc7a59ff3dbfef9cdd00cafd5720f0481f53ddf8eb`。
  该检查比仅比较帧计数更强，支持本夹具中省去 CPU 往返没有改变 VA 编码的画面或码流。
- 另跑 62 帧跟踪：62 次成功的 `vaCreateSurfaces` PRIME2 导入，descriptor 可见宽 2940、pitch 11776；
  未观察到 `gst_memory_map(dmabuf)` 或 VA 输入 image 映射。62 次 VA map 是压缩码流输出读取。
  启动阶段 3 次 derive/destroy 是能力探测，未映射输入像素。
- 明确的 `memory:DMABuf,format=DMA_DRM,drm-format=XR24` caps 在本机拒绝协商；
  普通 BGRx caps 携带真实 GstDmaBufMemory 能走 PRIME2。不能只凭 caps 名字推定复制行为。
  相关接口见 [GstDmaBufAllocator](https://gstreamer.freedesktop.org/documentation/allocators/gstdmabuf.html)、
  [GstVideoMeta](https://gstreamer.freedesktop.org/documentation/video/gstvideometa.html)及
  [GStreamer 1.28.2 VA allocator](https://github.com/GStreamer/gstreamer/blob/1.28.2/subprojects/gst-plugins-bad/gst-libs/gst/va/gstvaallocator.c)。

## 保留状态和后续边界

新增的 C/Python 文件都是独立验证入口；以 `-Wall -Wextra -Werror` 编译，最终源码另通过大尺寸画质冒烟。
测试从未替换正式 Agent；正式 ELF 哈希仍为
`f542fdb14c8c272c32ba22025247c01b31688dc222fd3f6bef31da4dd0d8fc20`。
本轮未安装驱动、修改电源/时钟/权限，也未推送新增验证文件。

正式接入还需要 GPU 帧类型、受限租约池、窗口 resize/close 的代际处理、重配/取消、静帧去重和可靠 CPU 回退，
并沿实际 Agent socket 重新做同帧发布测量。当前结果支持继续实现这条路径，尚不支持宣称正式采集到发布已减少 38%。
复现入口见 [DMA-buf 原型说明](../../tests/performance/README.md#私有-dma-buf-直达-gpu-的原型验证)。
