# Windows 系统声音（第一版）

在控制端「设备」页，打开某台设备下的「接收这台设备的系统声音」。默认关闭，按配对设备保存；开关即时生效，无需重连画面。接收所有应用播放的声音，不采集麦克风。音频属于设备，而不是某个应用窗口：多个接收窗口只播放一份；关闭画面后仍可收听，关闭声音开关、撤销配对或退出启动器可停止。

发布端目前为 Windows，播放端支持 Mac 和 Windows。Mac 更新后需完整运行 `apps/launcher/scripts/build-macos.sh`，并退出旧启动器后启动新包。新增的 Opus 原生依赖需要 CMake；Mac 缺少时先运行 `brew install cmake`，Windows 构建环境也需提供 CMake 和 C/C++ 编译器。无需独立显卡。

## 设计和边界

- CPAL 0.16 在默认 **输出** 设备上创建 WASAPI loopback 输入流。没有默认输入设备/麦克风回退。多个订阅共享一个采集器及编码器，最后一个订阅结束即停止；默认输出设备变化或失效后重新建立订阅。
- 采样转换为 48 kHz、双声道，10 ms 一包，Opus 目标 128 kbit/s、复杂度 5。处理常见浮点/整数格式、单声道/立体声和常见多声道下混。重采样保留分数相位，不逐包舍入。
- 实时回调只访问预分配的无锁 PCM 队列；不做编码、网络 I/O 或阻塞等待。编码/解码分别在专用线程完成。播放由启动器按设备统一管理，不在各个 WindowHost 中创建。
- 使用已有双向配对认证的 QUIC 连接，在独立双向流上传输 `audio.subscribe` / `audio.format` / `audio.frame` / `audio.ack`。音频帧不会进入视频的关键帧、几何版本或媒体队列。第一版是可靠 QUIC 流，**不是** UDP 音频 datagram；丢包仍可能引起本流的队头阻塞。
- 每条订阅最多 8 个未确认包；500 ms 无确认终止订阅。采集、编码包和播放队列均有上限；接收端用源时间戳与本地到达时间的差值丢弃相对积压超过 80 ms 的包，不要求两端系统时钟同步。播放启动/欠载时预缓冲 20 ms。播放设备变化或断线会自动重试，不影响视频订阅。
- Windows 共享关闭后停止音频；未配对设备不能请求。旧发布端未声明能力时显示升级提示。默认关闭的设备不会创建采集订阅。
- 不包含麦克风、语音通话、按应用隔离音频、静音远端扬声器或严格的音画同步时钟。可靠流在持续丢包下的体验、Mac 实机延迟及音画偏差仍需实测，不承诺不占带宽或 CPU。

## 调研依据

RustDesk 的 [audio_service.rs](https://github.com/rustdesk/rustdesk/blob/master/src/server/audio_service.rs) 使用系统播放采集和 Opus，按 10 ms 分包；[播放实现](https://github.com/rustdesk/rustdesk/blob/master/src/client/audio_playback.rs) 使用独立播放缓冲。[协议](https://github.com/rustdesk/rustdesk/blob/master/libs/base/protos/message.proto) 区分音频格式、音频帧和禁用音频选项。本实现独立编写，没有复制其 AGPL 实现。

微软的 [Loopback Recording](https://learn.microsoft.com/windows/win32/coreaudio/loopback-recording) 说明 WASAPI 系统回环不依赖硬件 Stereo Mix。CPAL 的 [WASAPI 后端](https://github.com/RustAudio/cpal/blob/v0.16.0/src/host/wasapi/device.rs) 在 render 设备上设置 loopback 标记。

UU 为闭源软件，没有验证到足够的官方音频实现说明。[非官方 Linux 互通项目](https://github.com/VallierDev/uu-remote-linux/blob/main/README.md) 描述了 WebRTC + Opus；这只能作为互通项目的依据，不能据此确认 UU 官方 Windows 客户端的采集 API、码率或缓存参数。

## 验证

- `cargo test -p paneferry-audio`：Opus 10 ms 立体声往返、44.1/48/96 kHz 转换和跨包采样相位。
- `cargo test -p paneferry-audio real_loopback -- --ignored --nocapture`：真实播放约一秒低音量测试音，回环采集并解码，检查非静音 PCM、共享采集和最后订阅关闭的资源释放。需 Windows 实机默认输出设备。
- Windows 发布端测试：未确认包上限、恢复确认后继续发送、关闭共享终止订阅。
- QUIC 测试：独立音频流的格式/帧/确认往返及顺序；原有视频阻塞时输入控制恢复测试仍执行。
- 启动器测试：每设备默认关闭/保存/关闭隔离、延迟包丢弃。另运行前端类型检查、构建及既有测试。
