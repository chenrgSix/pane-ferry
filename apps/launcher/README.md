# Tauri 启动器

Tauri 2 + Vue 3，通过固定 SSH 入口真实读取 Ubuntu 应用名称、图标及可接管窗口，显示可缓存的已安装应用、合并后的运行窗口和可选的自定义入口。使用打包的 Swift WindowHost 在 Mac 创建原生窗口。

## macOS 构建

需要已有 Rust、Node.js、Xcode Command Line Tools。运行 `cd apps/launcher && ./scripts/build-macos.sh`。脚本不安装系统软件。产物在 src-tauri/target/release/bundle/macos/，尚未签名公证；不要绕过系统安全警告。

浏览器单独 `npm run dev` 仅预览界面，不能执行 SSH。

## 首次连接

在终端核验真实主机指纹并建立 known_hosts 信任及可用 SSH 认证。本应用使用已有 SSH 密钥/agent 或项目临时控制连接，不处理密码、创建凭据或自动接受未知密钥。禁用 SSH 配置文件，避免 ProxyCommand/LocalCommand 隐式执行，因此使用直连 IPv4/DNS、用户名和端口，不支持 SSH alias、跳板、自定义 IdentityFile 或 IPv6。

只有密码登录时，可先在项目根目录运行 `bash scripts/connect-ssh.sh HOST USER 22`，直接在系统 SSH 中输入密码。启动器与 WindowHost 共用用户私有的 `~/.ssh/lan-remoteapp-%C` 控制连接；密码不进入应用、配置或日志，也不新增公钥授权。连接在最后一个客户端断开并空闲 10 分钟后结束。`--check` 可查看连接，`--disconnect` 可结束连接；主机校验始终严格，未知主机须先手工核验。

远端 Agent 运行后，填写主机并点击“连接并读取”，不要求先配置 apps.json。已安装应用的名称、图标与可选自定义入口按主机、账号、端口保存在本机，下次打开直接显示并自动读取运行状态。安装或卸载应用后点击“刷新应用”；“刷新运行状态”及返回启动器时仅刷新窗口和会话。点击已安装应用启动独立会话；运行列表统一点击“连接”，自动恢复已有会话或接管桌面窗口。连接偏好与目录缓存保存在本地 localStorage，不存秘密。

## 结果与恢复

“客户端已启动”不等于“远端窗口已呈现”。连接错误由 WindowHost 显示。目录来自远端真实桌面入口及可选自定义配置，发现失败会明确提示。未确认的启动请求 ID 保存在本地，失败或重启启动器后再次点击同一主机和应用仍复用该 ID，避免网络重试重复启动。原生启动会先返回远端会话 ID，再附着 WindowHost。

运行列表把同一桌面窗口与已有会话合并为一行，点击“连接”附着既有会话；重连失败会提示并刷新，不自动另开应用。已连接的会话须先在原 WindowHost 中断开，再刷新。命令行 `--session` 仍可使用。文本、截图和文件复制/粘贴仅由主动操作触发，不做后台同步；文件拖放、打印、URL、音频和摄像头关闭。

已在 Apple Silicon Mac 与 Ubuntu 26.04 上完成 xterm 基础链路实测；完整验证范围见 `docs/validation.md`，其他应用仍须逐项验收。
