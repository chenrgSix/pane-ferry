# 实现期 RWP/1 契约

下文定义RWP/1 本机业务接口。原生 QUIC 通过 RWN/2 多可靠流承载相同业务语义、二进制视频及控制水位，见[统一传输指南](native-transport.md)。

私有本机 Unix socket 与本机桥接的 stdin/stdout 承载 UTF-8 JSON Lines（Protobuf 留待兼容升级，当前不称 protobuf 实现）。客户端请求行最大 64 KiB，服务端事件行最大 16 MiB，PNG base64 全帧，宽高 1..4096，总像素 <= 8M；采样最短间隔 33ms，静止帧去重。所有请求与响应都必须包含 version:1，其他版本拒绝。每个消息 type 为以下字符串，version:1，服务端事件带 session_id、seq；窗口事件带 window_id（u32）与 geometry_revision（u64）。跨机传输只允许已配对的 QUIC 设备，不接受远端 shell 命令。

客户端第一条：{"type":"hello","version":1}；服务端 hello 包含 capabilities 字符串数组。
支持 H.264 时 hello 声明 native.h264；客户端在 launch/resume 之前发送 video.enable {codec:"h264"}，服务端回 video.ready {codec:"h264"}。未协商的连接继续使用 PNG，旧 hello 不变。协商只影响本连接，不能指定客户端编码程序、采集进程或任意编码参数。
客户端 app.list，服务端 app.list {apps:[{id,name,description,unavailable_reason?:null|string}]}。unavailable_reason 是现有窗口绑定已失效等稳定错误码，由客户端映射为提示；缺失或 null 表示未发现不可用原因，不保证后续启动必定成功。
新版 app.list 从 GIO 每次发现已安装桌面应用，并追加自定义入口。响应另有 discovery_supported:true、discovery_error:null|code；每项有 source:"desktop"|"custom" 和 icon_png:null|base64。图标仅为受限 PNG，应用图标最多 12KiB / 96px；控制端检查格式头、尺寸和长度。客户端不接收可执行路径或 argv；旧 Agent 没有 discovery_supported 时，新启动器明确提示需要更新。自动应用最多 512 个，自定义配置最多 256 个，控制响应行仍限制 16MiB。
客户端 window.list，服务端 window.list {windows:[{id,window_id,name,title,icon_png,minimized,controlled,session_id:null|string}],unavailable_reason:null|code}。最多 128 个当前用户的 X11/XWayland 窗口，原生 Wayland 不在本接口范围。只读查询不得截屏、改变焦点/尺寸或发送输入；单个窗口图标不超过 16KiB / 128px。自己的独立虚拟桌面不重复列入原桌面窗口。
接管从 window.list 返回的 id 仍使用 app.launch，不允许客户端传 PID、显示号、认证文件或任意 XID 作为接入目标。服务端注册并复核 XRes 客户 PID、同 UID、进程启动时间、可执行身份和选中窗口，仅该主窗口及关联弹窗进入会话。同 PID 的其他主窗口不属于这个会话。session_id 非空时从远程会话区重连，避免重复接管。
客户端 app.launch {app_id,request_id,backend:"native",attach?:bool,scale_factor?:1|2}；服务端 session.started {session_id,backend,display,scale_factor}。scale_factor 缺省为 1，表示界面像素密度，不是固定分辨率；独立会话在启动前设置 GTK/Qt 缩放环境，existing 沿用服务端配置的实际缩放。attach 默认 true，保留直接 WindowHost 启动并推流的兼容行为；启动器使用 attach:false，仅幂等创建会话，不占控制权，再以 session.resume 附着 WindowHost，避免连接交接竞争。相同 request_id 在已有控制端在线时仍返回同一会话；已退出会话的请求 ID 返回错误，不重新启动。
客户端 session.resume {session_id}；服务端 session.snapshot {session_id,windows:[完整 window.created 数据]}。
客户端 session.list；服务端 session.list {sessions:[{session_id,app_id,backend,display,controlled}]}，只返回尚未退出的会话，不抢占控制权。native controlled 为 true 时客户端应先从原宿主断开；原宿主关闭/断线后可从新启动器重新列出并附着。
客户端 session.detach，退出桥接但保留远端会话。

display 为本地 X11 显示号，允许 0。白名单配置 existing 时，app.launch 接入经批准的现有进程窗口，不新建应用；协议和 native 后端值保持不变。重复请求复用该进程会话。PID、Xauthority 等接入参数不属于客户端协议，进程退出或身份变化后不得继续输入或捕获。
窗口事件 window.created/window.metadata {window_id,parent_id:null|u32,title,role:"toplevel"|"dialog"|"popup",x,y,width,height,geometry_revision,visible:bool}；visible 缺失时按 true 兼容旧端。隐藏/重新映射同一 XID 只更新 visible，保留身份和几何版本，隐藏时暂停帧，重新显示时重发当前帧；真正销毁才发送 window.destroyed {window_id}。

`parent_id` 表示发布端核验后接纳的会话内关联关系，可跨进程；不能由控制端提交任意 PID 或窗口扩大范围。快照包含该会话全部窗口。接收端按设备 UUID、会话与窗口分发画面和输入；关联窗口的原生关闭按钮发送该窗口的 `window.close.request`，主窗口原生关闭仍表示断开。窗口级错误可附带 `window_id`，剪贴板失败只取消对应窗口的等待；旧端没有窗口编号时取消本连接尚未完成的复制，避免无限等待。

窗口元数据另带 scale_factor:1|2，旧端缺失时按 1；所有 width/height 都是实际渲染像素，Mac 逻辑窗口点数与它们分开管理。frame {window_id,geometry_revision,width,height,png:base64}。
视频帧使用同一 frame 类型，包含 codec:"h264",data:base64,stream_id:u64,key_frame:bool,encoded_width,encoded_height，省略 png。data 是 Annex B 对齐的单个 access unit；包最多 12 MiB，seq 仍按实际发送递增。encoded_width/height 仅允许在原尺寸为奇数时各补 1，解码前检查 SPS 尺寸一致，显示前裁掉补边。stream_id 在连接内标识每窗口的新编码流，流首包是含 SPS/PPS 的 IDR；重连、resize 和隐藏恢复都重新发关键帧。P 帧必须按发送顺序解码，不能合并已编码的依赖包；可在编码前合并原始画面、解码后合并待显示图像。编码失败回退帧有 png 和 video_error:"encoder_failed"，没有 codec 字段。
window.configure {window_id,width,height}；window.focus {window_id}；window.close.request {window_id}。
Mac 宿主红色关闭按钮只执行 session.detach 并退出宿主，不发送 window.close.request；后者保留为显式结束远端窗口的底层接口。用户也可在远端程序画面内选择关闭/退出。
window.configure 是请求任意合法像素宽高，不是把原窗口机械翻倍。宿主默认按当前内容区 × 本机 backing 像素密度持续计算；自动模式始终铺满，远端回包不改变本机窗口大小。手动分辨率模式固定远端像素尺寸，并按比例显示及映射输入。WM 内部窗口和从未显示过的辅助窗口不发布；已显示窗口隐藏后仍保留身份。迟到的隐藏/销毁窗口请求返回 window_unavailable，宿主等待元数据更新，不连续弹阻塞对话框。
input.pointer {window_id,x,y,button:0..7,pressed:bool}。button 0 只移动，4/5 滚轮。
input.key {window_id,keysym:u32,pressed:bool}，用于导航、快捷键与非文字按键。控制端输入法预编辑保留在本地；input.text.commit {window_id,text} 只传已经提交的 Unicode，不把物理字母键或输入法切换修饰键发往被控端。
声明 `input.authorization` 的 Agent，在接入原桌面 Xwayland 后额外发送 `input.authorization {state:"pending"|"ready"|"denied"|"paused"|"unavailable"}`，仍有当前 session_id / seq，只在状态变化时发送。授权属于 Agent 的共享输入会话，不随某个控制端断开而关闭。`desktop_authorization_pending` / `desktop_authorization_denied` / `desktop_input_paused` / `desktop_input_unavailable` 是受限输入错误码；控制端显示本地状态提示，不能为每次输入弹阻塞错误窗，也不能在授权恢复后重放等待期间的输入。恢复 token 不属于协议，不发送给客户端。普通 X11 / 独立 Xvfb 不发送此事件。
clipboard.paste {window_id,text} 由控制端主动粘贴触发。clipboard.copy {window_id,request_id} 由主动复制触发；成功返回 clipboard.data {window_id,request_id,text}。控制端只接受仍有效、窗口和请求 ID 相符的复制回包，其他事件不得修改其剪贴板。没有通用 clipboard.read 接口或后台剪贴板同步。
文字提交与旧版文本剪贴板接口只支持非空 UTF-8 纯文本，单次上限 8192 字节，禁止 NUL，超限报错而非截断。Agent 用 X11 selection 交付文字；XRes 校验请求方/selection owner 属于目标应用或经进程身份核验的桌面合成器桥接进程。原生 X11 等待读取完成；Xwayland 桥接完成数据请求后保留 selection source，以支持异步读取，后续文字/复制操作或断开时替换并清理。桥接读取不代表应用已经完成语义上的粘贴。复制先清除旧 selection 再触发本次 Ctrl+C，未产生新内容则超时失败，不返回之前的剪贴板。所有未完成操作有 3 秒超时；不会恢复或保存操作前的被控端剪贴板。
图片、文件/文件夹及剪切使用 `clipboard.bundle` 能力与独立 QUIC 可靠流，完整消息、限制、INCR 和缓存规则见[剪贴板说明](clipboard.md)。新旧文本接口保持兼容，没有后台同步。

新建自有实例使用不连接桌面 IM daemon 的 GTK/Qt 输入上下文，组合仍在控制端完成；现有进程的输入法环境不被修改。普通 X11 应用保持快速路径；Firefox 对新剪贴板快照增加 16ms 广告时间，并为快捷键转换保留 4ms 节奏，提前读取也会计入交付状态。selection 的读取确认不等同于编辑器的语义提交确认，客户端不得将其解释为网页已经接受输入。
服务端 error {code,message}，无敏感数据。会话退出 session.exited。

已退出的原进程返回 existing_process_unavailable；已结束/回收的会话返回 session_exited / session_expired。明确拒绝启动后，启动器可清除待确认请求 ID，再由用户触发新启动；网络错误、超时与未知 request_failed 必须保留 ID。客户端只显示本地映射的错误码解释，不显示任意服务端详情。

需要桌面密钥服务兼容策略的应用，在本地总线或受限代理不可用时返回 `desktop_secrets_unavailable`。这是启动前的明确拒绝，不保留新会话。该策略由发布端本地目录决定，客户端不能提交总线地址、代理规则或修改启动策略。

视频/PNG 编码与 socket 写出由独立线程执行，不持有会话输入锁。待编码的同一窗口旧原始帧被最新帧替换，新帧仍排在此前的元数据事件之后；seq 在实际发送时生成。原始像素去重包含 geometry_revision，隐藏/重现会重发当前帧。采样最短间隔 33ms，实际帧率受编码、网络及控制端限制；已实现 H.264 over QUIC，尚未实现 WebRTC 视频轨道。

Catalog 来自该 Linux 用户安装的桌面入口，以及可选的 ~/.config/lan-remoteapp/apps.json 自定义入口。Exec 字段由发布器解析、启动前重新确认对应桌面入口；自定义配置只接受固定绝对 executable + argv 数组。客户端不能传 executable 或命令字符串。原生后端是唯一实现，其他 backend 值在协议解析时拒绝。网络组件通过本机私有 socket 访问发布器，不执行远端 shell 命令。完整行为见 [应用与窗口发现](application-discovery.md)。

窗口元数据可附加 `scale_mode`：`source` 表示沿用已有应用缩放，`requested` 表示只设置了启动缩放偏好，缺省表示未报告。`scale_factor` 仍用于兼容的窗口初始尺寸换算，不能单独证明应用实际 DPI；接收端依据成功解码的帧尺寸显示实际像素。
