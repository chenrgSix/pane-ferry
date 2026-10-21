# 实现期 RWP/1 契约

P1 使用 SSH stdin/stdout 承载 UTF-8 JSON Lines（Protobuf 留待兼容升级，当前不称 protobuf 实现）。客户端请求行最大 64 KiB，服务端事件行最大 16 MiB，PNG base64 全帧，宽高 1..4096，总像素 <= 8M；采样最短间隔 33ms，静止帧去重。所有请求与响应都必须包含 version:1，其他版本拒绝。每个消息 type 为以下字符串，version:1，服务端事件带 session_id、seq；窗口事件带 window_id（u32）与 geometry_revision（u64）。SSH 固定远端命令为 `.local/bin/remoteapp-agent bridge`，固定 argv 本地使用 /usr/bin/ssh -T -oBatchMode=yes -oStrictHostKeyChecking=yes -p PORT -l USER -- HOST REMOTE_COMMAND。用户先手工建立可信 known_hosts 和正常 SSH 登录，不自动接受新密钥。

客户端第一条：{"type":"hello","version":1}；服务端 hello 包含 capabilities 字符串数组。
支持 H.264 时 hello 声明 native.h264；客户端在 launch/resume 之前发送 video.enable {codec:"h264"}，服务端回 video.ready {codec:"h264"}。未协商的连接继续使用 PNG，旧 hello 不变。协商只影响本连接，不能指定客户端编码程序、采集进程或任意编码参数。

可选 WebRTC runtime 可用且支持编码时，hello 另声明 native.webrtc.h264。客户端可在 launch/resume 前改发 webrtc.enable {}，服务端回 video.ready {codec:"h264",transport:"webrtc"}。每窗口一条 sendonly H.264 视频媒体轨道，MID 为十进制 window_id。服务端 webrtc.offer {sdp} / 客户端 webrtc.answer {sdp} 及双向 webrtc.candidate {candidate,mid} 均走既有 SSH；SDP 上限 48KiB，候选上限 2048 字节，MID 为 1–10 位数字，拒绝额外字段、NUL 及候选行注入。保留标准 DTLS 指纹核验，只使用 ICE host candidates。

WebRTC 的 H.264 AU 不以 base64 发到 SSH。对应 frame 元数据另有 transport:"webrtc"、frame_id，连同长度/CRC32 放入标准未注册 SEI，经同一 RTP 媒体轨道交付；移除自有 SEI 后，原始 NAL 内容不变。控制消息 seq 与媒体帧顺序独立，媒体到达不能推进控制事件的 lastSeq，避免丢掉尚未到达的 resize。尺寸通知尚未到达时顺序解码，只保留最新已解码画面等待匹配几何版本；单 AU 上限 12MiB。

服务端 webrtc.state {state} / webrtc.failed {} 通知媒体状态，webrtc.pulse {frames:{window_id:last_frame_id}} 每秒报告已发帧进度；静止帧 ID 不变。客户端可发 video.fallback {}，销毁媒体 peer、清除编码缓存并强制刷新，在同一会话重新发 SSH H.264 关键帧。Mac auto 模式有建立超时和未交付新帧的检测；强制 webrtc 模式明确报错，不静默回退。媒体库、修复缓存和检测细节见 [WebRTC 指南](webrtc.md)。

客户端 app.list，服务端 app.list {apps:[{id,name,description,unavailable_reason?:null|string}]}。unavailable_reason 是现有窗口绑定已失效等稳定错误码，由客户端映射为提示；缺失或 null 表示未发现不可用原因，不保证后续启动必定成功。
客户端 app.launch {app_id,request_id,backend:"native",attach?:bool,scale_factor?:1|2}；服务端 session.started {session_id,backend,display,scale_factor}。scale_factor 缺省为 1，表示界面像素密度，不是固定分辨率；独立会话在启动前设置 GTK/Qt 缩放环境，existing 沿用服务端配置的实际缩放。attach 默认 true，保留直接 WindowHost 启动并推流的兼容行为；启动器使用 attach:false，仅幂等创建会话，不占控制权，再以 session.resume 附着 WindowHost，避免连接交接竞争。相同 request_id 在已有控制端在线时仍返回同一会话；已退出会话的请求 ID 返回错误，不重新启动。
客户端 session.resume {session_id}；服务端 session.snapshot {session_id,windows:[完整 window.created 数据]}。
客户端 session.list；服务端 session.list {sessions:[{session_id,app_id,backend,display,controlled}]}，只返回尚未退出的会话，不抢占控制权。native controlled 为 true 时客户端应先从原宿主断开；原宿主关闭/断线后可从新启动器重新列出并附着。
客户端 session.detach，退出桥接但保留远端会话。

display 为本地 X11 显示号，允许 0。白名单配置 existing 时，app.launch 接入经批准的现有进程窗口，不新建应用；协议和 native 后端值保持不变。重复请求复用该进程会话。PID、Xauthority 等接入参数不属于客户端协议，进程退出或身份变化后不得继续输入或捕获。
窗口事件 window.created/window.metadata {window_id,parent_id:null|u32,title,role:"toplevel"|"dialog"|"popup",x,y,width,height,geometry_revision,visible:bool}；visible 缺失时按 true 兼容旧端。隐藏/重新映射同一 XID 只更新 visible，保留身份和几何版本，隐藏时暂停帧，重新显示时重发当前帧；真正销毁才发送 window.destroyed {window_id}。
窗口元数据另带 scale_factor:1|2，旧端缺失时按 1；所有 width/height 都是实际渲染像素，Mac 逻辑窗口点数与它们分开管理。frame {window_id,geometry_revision,width,height,png:base64}。
视频帧使用同一 frame 类型，包含 codec:"h264",data:base64,stream_id:u64,key_frame:bool,encoded_width,encoded_height，省略 png。data 是 Annex B 对齐的单个 access unit；包最多 12 MiB，seq 仍按实际发送递增。encoded_width/height 仅允许在原尺寸为奇数时各补 1，解码前检查 SPS 尺寸一致，显示前裁掉补边。stream_id 在连接内标识每窗口的新编码流，流首包是含 SPS/PPS 的 IDR；重连、resize 和隐藏恢复都重新发关键帧。P 帧必须按发送顺序解码，不能合并已编码的依赖包；可在编码前合并原始画面、解码后合并待显示图像。编码失败回退帧有 png 和 video_error:"encoder_failed"，没有 codec 字段。
window.configure {window_id,width,height}；window.focus {window_id}；window.close.request {window_id}。
window.configure 是请求任意合法像素宽高，不是把原窗口机械翻倍。宿主默认按当前内容区 × 本机 backing 像素密度持续计算；自动模式始终铺满，远端回包不改变本机窗口大小。手动分辨率模式固定远端像素尺寸，并按比例显示及映射输入。WM 内部窗口和从未显示过的辅助窗口不发布；已显示窗口隐藏后仍保留身份。迟到的隐藏/销毁窗口请求返回 window_unavailable，宿主等待元数据更新，不连续弹阻塞对话框。
input.pointer {window_id,x,y,button:0..7,pressed:bool}。button 0 只移动，4/5 滚轮。
input.key {window_id,keysym:u32,pressed:bool}，用于导航、快捷键与非文字按键。控制端输入法预编辑保留在本地；input.text.commit {window_id,text} 只传已经提交的 Unicode，不把物理字母键或输入法切换修饰键发往被控端。
clipboard.paste {window_id,text} 由控制端主动粘贴触发。clipboard.copy {window_id,request_id} 由主动复制触发；成功返回 clipboard.data {window_id,request_id,text}。控制端只接受仍有效、窗口和请求 ID 相符的复制回包，其他事件不得修改其剪贴板。没有通用 clipboard.read 接口或后台剪贴板同步。
文字提交与剪贴板均只支持非空 UTF-8 纯文本，单次上限 8192 字节，禁止 NUL，超限报错而非截断。Agent 用 X11 selection 交付文字；XRes 校验请求方/selection owner 属于目标应用或经进程身份核验的桌面合成器桥接进程。原生 X11 等待读取完成；Xwayland 桥接完成数据请求后保留 selection source，以支持异步读取，后续文字/复制操作或断开时替换并清理。桥接读取不代表应用已经完成语义上的粘贴。复制先清除旧 selection 再触发本次 Ctrl+C，未产生新内容则超时失败，不返回之前的剪贴板。所有未完成操作有 3 秒超时；不会恢复或保存操作前的被控端剪贴板。
服务端 error {code,message}，无敏感数据。会话退出 session.exited。

已退出的原进程返回 existing_process_unavailable；已结束/回收的会话返回 session_exited / session_expired。明确拒绝启动后，启动器可清除待确认请求 ID，再由用户触发新启动；网络错误、超时与未知 request_failed 必须保留 ID。客户端只显示本地映射的错误码解释，不显示任意服务端详情。

视频/PNG 编码与 socket 写出由独立线程执行，不持有会话输入锁。待编码的同一窗口旧原始帧被最新帧替换，新帧仍排在此前的元数据事件之后；seq 在实际发送时生成。原始像素去重包含 geometry_revision，隐藏/重现会重发当前帧。采样最短间隔 33ms，实际帧率受编码、网络及控制端限制；默认 H.264 over SSH，可显式协商 WebRTC 视频轨道。

Catalog 用 Linux 用户私有 JSON 配置 ~/.config/lan-remoteapp/apps.json，只接受经审批固定绝对 executable + argv 数组，客户端不能传 executable。原生后端是唯一实现，其他 backend 值在协议解析时拒绝。固定远端 Agent 可由 shell 执行但无客户端参数插值。
