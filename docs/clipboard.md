# 控制端与 Ubuntu 剪贴板

主动复制、剪切和粘贴使用本项目的 RWP 控制消息及 SSH 二进制数据流，不依赖 Xpra、系统输入法或第三方剪贴板同步。仅在用户操作时读取内容，没有后台轮询。输入法预编辑仍只留在控制端。

## 支持内容

| 内容 | Mac → Ubuntu | Ubuntu → Mac |
| --- | --- | --- |
| UTF-8 文本 | Command+V | Command+C / 编辑菜单 |
| 截图、PNG 图片 | 原始 PNG；TIFF 截图转 PNG，保持像素尺寸 | X11 `image/png` 转为 Mac 图片剪贴板 |
| 普通文件、多个文件 | 传输完整文件，向远端应用提供当地文件 URI | 将当前应用复制的文件快照传回 Mac，写入原生文件 URL |
| 文件夹、空文件 | 保留相对层级、名称与完整内容 | 同左 |

单次文本最多 8192 字节；图片最多 64 MiB、6400 万像素；文件合计最多 1 GiB，最多 4096 个文件/目录条目。符号链接、特殊设备文件和非 UTF-8 文件名暂不支持。接收端不兼容的名称冲突会拒绝整个操作，文件不会静默覆盖。

接收应用需要支持相应类型的粘贴。例如浏览器网页可在 paste 事件中接收实际图片/文件对象；文件管理器可读取 `text/uri-list`、`x-special/gnome-copied-files`。只接收文字的输入框不会自动获得图片附件能力。当前窗口链路支持 X11 / XWayland，原生 Wayland 应用范围不变。

Command+X 支持可编辑文本和图片的原生剪切：先读取并验证选中内容，再发送远端 Ctrl+X，并读取实际剪切产生的新内容。空选区、超限和不支持的格式在剪切前失败。终端禁用此入口，避免 Ctrl+X 的其他含义。跨端文件移动尚未实现；文件使用复制，文件剪切请求会在触发 Ctrl+X 前拒绝，保留原文件。

## 协议与数据流

Agent 的 hello 广告 `clipboard.bundle` 能力。新增消息：

```
clipboard.copy.bundle {window_id,request_id,cut?:bool}
clipboard.paste.bundle {window_id,transfer_id}
clipboard.bundle {window_id,request_id,transfer_id}  // 带 session_id、seq 的服务端事件
```

文本回包沿用 `clipboard.data`；旧客户端的 `clipboard.copy` / `clipboard.paste` 不变。新版客户端连接旧 Agent 时保留文字功能，不以物理按键替代未知的图片或文件接口。

大内容通过同一 SSH 身份、严格主机指纹校验及复用连接的独立 channel 传输：固定的 `remoteapp-agent clipboard-upload` / `clipboard-download ID`。数据是最多 1 MiB 的 JSON manifest 行，随后按 manifest 顺序传输各普通文件的精确字节数。文件使用 64 KiB 缓冲流式读写；没有把二进制塞进画面 JSON 队列，也没有开放网络监听端口。每个快照的随机 ID 由接收端生成，远端命令不插入文件名或任意路径。

manifest 包含 `kind: image|files` 和 `entries: [{path,size,directory}]`。只接受相对路径，拒绝绝对路径、空段、`..`、反斜杠、换行、重复条目、文件/目录冲突、缺失父目录和超过总大小的声明。收发双方检查长度、类型、条目数及中断流；未完成下载不会改写控制端剪贴板。

X11 两个方向均支持 ICCCM INCR 大 selection；每次属性块最多 32 KiB。TARGETS 协商优先文件、图片，再选择文本。读取及交付用 XRes 绑定当前受控应用和经核验的桌面桥接 PID，不提供读取任意桌面剪贴板的通用接口。取得新 selection 的等待最多 3 秒，图片/文件 selection 阶段最多 30 秒，SSH bulk 阶段最多 300 秒。

控制端等待复制/剪切结果时暂存后续输入，复制成功后才处理粘贴意图，避免粘贴旧内容；失败则取消对应的排队粘贴，其他输入仍可继续。回包须匹配 session、窗口、请求 ID 和有效期。如果用户在等待期间又在本机复制其他内容，旧回包不会覆盖较新的本机剪贴板。

## 缓存

文件剪贴板保存文件引用，需要在原生粘贴完成前保持本机快照。Ubuntu 快照位于 `~/.cache/lan-remoteapp/clipboard`，Mac 位于 `~/Library/Caches/LAN RemoteApp/Clipboard`；目录仅当前用户可访问，接收文件为 0600。缓存限制约 4 GiB，新操作会清理七天前的快照；Mac 当前剪贴板仍引用的快照受到保护。下载后的文件可通过应用正常粘贴到用户选定位置，不会自动删除源文件。不要把临时缓存当作长期保存位置。

源文件的类型/大小或内容在读取期间变化时可失败；不会截断后当作传输成功。所有缓存属于当前 SSH 用户，不能把它当作同 UID 应用之间的强安全隔离。日志不记录剪贴板正文、文件内容或画面。

## 验证

- `controller_input_smoke.py`：真实 AppKit 文本输入、Command+X、按键释放、独立命名粘贴板中的 PNG/TIFF 与原生文件 URL、二进制接收和路径校验。
- `clipboard_ordering_smoke.py`：真实 AppDelegate 的快速剪切后粘贴、失败后取消粘贴、拒绝无关回包、保留较新的本机剪贴板；不连接用户应用。
- `mac_clipboard_bulk_smoke.py --host HOST --user USER`：生成的测试 PNG/文件经实际 Mac ↔ Ubuntu SSH 通道往返，验证原始字节并清理本轮远端快照。
- `rich_clipboard_smoke.py --agent PATH`：隔离 HOME/Xvfb/GTK 中，超过 1 MiB PNG 的双向 INCR、文件夹/中文文件名/空文件及内容哈希、文件剪切拒绝、路径穿越与中断流。
- `browser_text_smoke.py --agent PATH`：独立 Firefox 网页收到真实图片和文件对象，内容哈希一致；8 轮中文复制/剪切/粘贴/跳转，标签关闭、连续提交与 H.264 重连。

协议参考：[ICCCM selection / INCR](https://www.x.org/releases/X11R7.7/doc/xorg-docs/icccm/icccm.pdf)、[Apple NSPasteboard](https://developer.apple.com/documentation/AppKit/NSPasteboard)、[FreeDesktop Clipboard Spec](https://specifications.freedesktop.org/clipboard/latest/)。
