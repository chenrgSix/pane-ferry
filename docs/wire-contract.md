# 实现期 RWP/1 契约

P1 使用 SSH stdin/stdout 承载 UTF-8 JSON Lines（Protobuf 留待兼容升级，当前不称 protobuf 实现）。客户端请求行最大 64 KiB，服务端事件行最大 16 MiB，PNG base64 全帧，宽高 1..4096，总像素 <= 8M；默认 10fps，静止帧去重。所有请求与响应都必须包含 version:1，其他版本拒绝。每个消息 type 为以下字符串，version:1，服务端事件带 session_id、seq；窗口事件带 window_id（u32）与 geometry_revision（u64）。SSH 固定远端命令为 `.local/bin/remoteapp-agent bridge`，固定 argv 本地使用 /usr/bin/ssh -T -oBatchMode=yes -oStrictHostKeyChecking=yes -p PORT -l USER -- HOST REMOTE_COMMAND。用户先手工建立可信 known_hosts 和正常 SSH 登录，不自动接受新密钥。

客户端第一条：{"type":"hello","version":1}；服务端 hello 包含 capabilities 字符串数组。
客户端 app.list，服务端 app.list {apps:[{id,name,description}]}。
客户端 app.launch {app_id,request_id,backend:"native",attach?:bool}；服务端 session.started {session_id,backend,display}。attach 默认 true，保留直接 WindowHost 启动并推流的兼容行为；启动器使用 attach:false，仅幂等创建会话，不占控制权，再以 session.resume 附着 WindowHost，避免连接交接竞争。相同 request_id 在已有控制端在线时仍返回同一会话；已退出会话的请求 ID 返回错误，不重新启动。
客户端 session.resume {session_id}；服务端 session.snapshot {session_id,windows:[完整 window.created 数据]}。
客户端 session.list；服务端 session.list {sessions:[{session_id,app_id,backend,display,controlled}]}，只返回尚未退出的会话，不抢占控制权。native controlled 为 true 时客户端应先从原宿主断开；原宿主关闭/断线后可从新启动器重新列出并附着。
客户端 session.detach，退出桥接但保留远端会话。
窗口事件 window.created/window.metadata {window_id,parent_id:null|u32,title,role:"toplevel"|"dialog"|"popup",x,y,width,height,geometry_revision,visible:bool}；visible 缺失时按 true 兼容旧端。隐藏/重新映射同一 XID 只更新 visible，保留身份和几何版本，隐藏时暂停帧，重新显示时重发当前帧；真正销毁才发送 window.destroyed {window_id}。
frame {window_id,geometry_revision,width,height,png:base64}。
window.configure {window_id,width,height}；window.focus {window_id}；window.close.request {window_id}。
input.pointer {window_id,x,y,button:0..7,pressed:bool}。button 0 只移动，4/5 滚轮。
input.key {window_id,keysym:u32,pressed:bool}，keysym 为 X11 keysym，Mac 端做键映射；input.text.commit 暂不支持 Unicode IME，必须返回 unsupported，不假称实现。
服务端 error {code,message}，无敏感数据。会话退出 session.exited。

Catalog 用 Linux 用户私有 JSON 配置 ~/.config/lan-remoteapp/apps.json，只接受经审批固定绝对 executable + argv 数组，客户端不能传 executable。原生后端是唯一实现，其他 backend 值在协议解析时拒绝。固定远端 Agent 可由 shell 执行但无客户端参数插值。
