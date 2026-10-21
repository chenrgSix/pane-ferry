# 原生 WebRTC H.264

WebRTC 使用 libdatachannel 的视频媒体轨道：H.264 经 RTP 分片、DTLS-SRTP 加密后通过 ICE 选出的 UDP 路径传输。没有浏览器、第二次编码或以 DataChannel 承载视频。SSH 继续负责认证、SDP/ICE 信令、窗口状态、键鼠与主动双向文本剪贴板。

## 构建

两端均先构建 runtime，固定使用 libdatachannel v0.24.4，提交 `8e330edebfe766b44f59be427e0454fd18964d4e`。需要 CMake 3.21+、Ninja、C++17 编译器和 OpenSSL 开发文件。脚本不安装系统包、不改防火墙。

```sh
python3 scripts/build-webrtc.py
```

首次运行会克隆官方源码及子模块；也可用 `--source /ABS/libdatachannel` 指定已检出的同一提交。输出在 `target/webrtc/`，包括动态库、许可证和固定提交号。`--openssl-root /ABS/openssl` 可选择已有开发文件；Apple Silicon 默认使用 `/opt/homebrew/opt/openssl@3`，Intel Mac 需指定自己的路径。macOS 静态链接 OpenSSL，Linux 使用系统 OpenSSL runtime。

Ubuntu 安装在 Agent 二进制旁的用户库目录：

```sh
mkdir -p "$HOME/.local/lib/lan-remoteapp"
install -m 755 target/webrtc/libremoteapp_webrtc.so "$HOME/.local/lib/lan-remoteapp/"
cp -R target/webrtc/licenses "$HOME/.local/lib/lan-remoteapp/"
cp target/webrtc/libdatachannel-revision.txt "$HOME/.local/lib/lan-remoteapp/"
cp target/webrtc/THIRD_PARTY_NOTICES.md "$HOME/.local/lib/lan-remoteapp/"
```

Agent 编译安装方式见 [Linux Agent 指南](linux-agent.md)。安装后需重启 Agent 才会重新检测组件；停止服务前保存其自建会话里的工作。

Mac 的 `apps/mac-windowhost/scripts/build-app.sh` 自动打包 `target/webrtc` 中的 runtime；也可通过 `REMOTEAPP_WEBRTC_RUNTIME=/ABS/RUNTIME` 指定目录。启动器的 `apps/launcher/scripts/build-macos.sh` 会将完整 WindowHost.app 内嵌进最终应用。库位于 WindowHost.app 的 `Contents/Frameworks`，许可证在 `Contents/Resources/WebRTC`。纯 `swift build` 的裸二进制可通过 `REMOTEAPP_WEBRTC_LIBRARY=/ABS/libremoteapp_webrtc.dylib` 加载测试库。

## 传输选择与恢复

WindowHost 支持 `--video-transport ssh|webrtc|auto`，当前默认 `ssh`。`webrtc` 为强制测试模式，不能建立媒体连接时明确失败，禁止把 SSH 回退记作 WebRTC 测试成功。`auto` 在两端组件可用时选择 WebRTC，否则使用 SSH；建立媒体连接超时、连接失败或发送端有新帧而接收端持续未收到时，保持同一个 SSH 会话并回退 H.264 over SSH。窗口副标题显示实际传输。

只使用本机 ICE host candidates，不配置 STUN、TURN、公网转发或外部信令服务。媒体需要两端可达的 UDP 路径；不能通过时 `auto` 回退。测试可用 `REMOTEAPP_WEBRTC_BIND` 固定本机媒体接口；这是开发诊断设置，普通连接无需填写。

## 画质与有界恢复

原来的采集、像素尺寸、自动适应、UI 缩放、颜色转换、OpenH264 编码参数及 VideoToolbox 解码保持不变。只为原始编码 AU 加入标准未注册 SEI 元数据，再交给 RTP；接收端移除自有 SEI，原有 NAL 内容保持一致，不降低分辨率或重新压缩。

同一 AU 携带会话、窗口、几何版本、流 ID、帧 ID、长度及 CRC32；SRTP 负责认证和加密。完整帧立即解码，仅在有缺片时短暂等待重传。NACK 请求缺包，60ms 内未修复则请求 IDR；丢失依赖帧后等待关键帧，避免显示损坏画面。每帧最多 12MiB，重组缓存总量 32MiB；Swift 编码帧队列最多 64 帧 / 24MiB，超限会重建解码依赖并请求 IDR。不会任意跳过中间 P 帧继续解码。

窗口尺寸控制消息仍走 SSH，媒体可能先到；接收端先顺序解码，保留最新已解码画面，等待匹配的几何版本。静止窗口的进度值不变，不会因没有新画面而误判断流。

## 验证

- `native/webrtc` 的 wire / receiver 测试覆盖 SEI、完整性、分片重排、重复、序号回绕、重传、丢片恢复和缓存上限。
- `tests/e2e/webrtc_transport_smoke.py` 使用真正的 ICE / DTLS / SRTP / UDP 连接核对媒体字节与多窗口协商；可用 `--packets` 验证实际 Agent 编码包。
- `video_stream_smoke.py` 与 `existing_window_smoke.py` 的 `--webrtc-library` 运行真实媒体路径，覆盖输入、奇数尺寸、重连及主动双向剪贴板。
- [全链路测量方法](../tests/performance/README.md)要求实际对应输入画面、UDP 候选对与硬件解码，另有建立失败和运行中断流的自动回退测试。

当前覆盖本机与指定 Ubuntu 的直连测试。弱网拥塞、任意丢包率、跨网络切换、休眠和长期稳定性尚未完整验收。
