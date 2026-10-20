# 实现期 RWP/1 契约

P1 使用 SSH stdin/stdout 承载 UTF-8 JSON Lines（Protobuf 留待兼容升级，当前不称 protobuf 实现）。客户端请求行最大 64 KiB，服务端事件行最大 16 MiB，PNG base64 全帧，宽高 1..4096，总像素 <= 8M；默认 10fps，静止帧去重。所有请求与响应都必须包含 version:1，其他版本拒绝。每个消息 type 为以下字符串，version:1，服务端事件带 session_id、seq；窗口事件带 window_id（u32）与 geometry_revision（u64）。SSH 固定远端命令为 `.local/bin/remoteapp-agent bridge`，固定 argv 本地使用 /usr/bin/ssh -T -oBatchMode=yes -oStrictHostKeyChecking=yes -p PORT -l USER -- HOST REMOTE_COMMAND。用户先手工建立可信 known_hosts 和正常 SSH 登录，不自动接受新密钥。

客户端第一条：{"type":"hello","version":1}；服务端 hello 包含 capabilities 字符串数组。
客户端 app.list，服务端 app.list {apps:[{id,name,description}]}。
客户端 app.launch {app_id,request_id,backend:"native"|"xpra"}；服务端 session.started {session_id,backend,display}。
客户端 session.resume {session_id}；服务端 session.snapshot {session_id,windows:[完整 window.created 数据]}。
客户端 session.detach，退出桥接但保留远端会话。
窗口事件 window.created/window.metadata {window_id,parent_id:null|u32,title,role:"toplevel"|"dialog"|"popup",x,y,width,height,geometry_revision}；window.destroyed {window_id}。
frame {window_id,geometry_revision,width,height,png:base64}。
window.configure {window_id,width,height}；window.focus {window_id}；window.close.request {window_id}。
input.pointer {window_id,x,y,button:0..7,pressed:bool}。button 0 只移动，4/5 滚轮。
input.key {window_id,keysym:u32,pressed:bool}，keysym 为 X11 keysym，Mac 端做键映射；input.text.commit 暂不支持 Unicode IME，必须返回 unsupported，不假称实现。
服务端 error {code,message}，无敏感数据。会话退出 session.exited。

Catalog 用 Linux 用户私有 JSON 配置 ~/.config/lan-remoteapp/apps.json，只接受经审批固定绝对 executable + argv 数组，客户端不能传 executable。P0 app.launch 返回 Xpra display，Mac 用严格 OpenSSH 的 xpra attach ssh://user@host:port/DISPLAY，禁止 auto Paramiko、禁文件/打印/音视频转发。固定远端 Agent 可由 shell 执行但无客户端参数插值。
