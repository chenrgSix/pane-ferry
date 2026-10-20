# Tauri 启动器

Tauri 2 + Vue 3，通过 Rust 固定 SSH 入口读取应用目录。P0 使用 Xpra 原生客户端；P1 使用打包的 Swift WindowHost，不依赖 XQuartz。

## macOS 构建

需要已有 Rust、Node.js、Xcode Command Line Tools。运行 `cd apps/launcher && ./scripts/build-macos.sh`。脚本不安装系统软件。产物在 src-tauri/target/release/bundle/macos/，尚未签名公证；不要绕过系统安全警告。

P0 另需官方 Xpra Mac 客户端，界面默认路径可调整。P1 不需要 Xpra。浏览器单独 `npm run dev` 仅预览界面，不能执行 SSH。

## 首次连接

在终端核验真实主机指纹并建立 known_hosts 信任及可用 SSH 认证。本应用只用已有标准 SSH 密钥/agent，不处理密码、创建凭据或自动接受未知密钥。禁用 SSH 配置文件，避免 ProxyCommand/LocalCommand 隐式执行，因此使用直连 IPv4/DNS、用户名和端口，不支持 SSH alias、跳板、自定义 IdentityFile 或 IPv6。

远端 Agent 运行、应用白名单配置完成后，填写主机并读取目录，点击应用。连接偏好只保存在本地 localStorage，不存秘密。

## 结果与恢复

“客户端已启动”不等于“远端窗口已呈现”。连接错误由 Xpra/WindowHost 显示。目录由服务端白名单返回。未确认的启动请求 ID 保存在本地，失败或重启启动器后再次点击同一主机、应用和模式仍复用该 ID，避免网络重试重复启动。原生启动会先返回远端会话 ID，再附着 WindowHost。

重新读取同一主机可看到“运行中的会话”，点击“重新连接”附着既有会话，不重新启动应用。已连接的原生会话须先在原 WindowHost 中断开，再刷新会话列表。命令行 `--session` 方式仍可使用。默认关闭文件、打印、URL、音视频、摄像头和剪贴板转发。

真实 Mac/Ubuntu 联调尚未执行；前端构建不代表原生应用验收。
