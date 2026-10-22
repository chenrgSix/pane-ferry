# Ubuntu 应用与窗口发现

启动器分开显示：

- **已安装应用**：通过系统 GIO 读取桌面入口、本地化名称和真实图标。首次读取成功后，启动器按配对设备 UUID 在本机缓存名称和图标，重开直接显示；安装、卸载或修改入口后点击“刷新应用”更新。刷新失败保留上一次成功结果。
- **运行中的窗口**：将可接入的桌面窗口和 PaneFerry 远程会话合并为一份列表。统一点击“连接”：已有会话直接重连，其余桌面窗口才接管，保留原进程、内容和登录状态。同一个窗口和会话不重复显示；同一应用的多个独立会话仍分别保留。

原有 `~/.config/lan-remoteapp/apps.json` 是可选的**自定义入口**，用于固定参数、独立测试资料或旧的进程接入配置，不再代替系统应用列表。自定义配置在服务启动时加载，并随应用目录缓存在控制端；“刷新应用”才重新请求目录，窗口和会话状态不使用持久缓存。没有 apps.json 也能启动 Agent。

缓存最多保存 8 个设备目录并限制总大小；存储不可用时保留本次启动的内存缓存，并在界面提示。启动器恢复上次主机后自动读取运行状态；连接主机与刷新运行状态都会复用应用缓存，避免重复扫描图标。缓存仅包含显示信息，启动时仍由 Agent 重新解析和校验应用入口，不根据缓存执行程序路径。

## 来源与图标

使用 GIO / GioUnix 的 DesktopAppInfo，不使用硬编码应用数组。GIO 处理 XDG 目录优先级、用户覆盖、Hidden、NoDisplay、桌面环境可见性、TryExec 和本地化名称。没有显式设置 XDG_DATA_DIRS 时，补充存在的 Snap / Flatpak 导出目录。没有桌面入口的 CLI 工具不自动列入图形应用。

文件图标和主题图标在 Ubuntu 端通过 GdkPixbuf 转成 PNG；不把 SVG / XML 或 Ubuntu 文件路径直接送到 WebView。优先输出 96px，复杂图标降为 64px / 48px，使应用图标不超过 12KiB；窗口图标最多 16KiB。控制端检查格式头、尺寸和长度，只构造固定 PNG data URL。缺失或解码失败时显示名称首字母。

需要系统 Python3 GI、GIO / GioUnix、GTK3、GdkPixbuf 和图片解码器；Ubuntu 26 的 SVG 解码还使用系统 Glycin 配置。本机 Ubuntu 已具备这些组件。缺少组件时明确报告发现失败，保留自定义入口，不把静态目录当作系统扫描成功。`paneferry-agent check` 会报告应用与图标数量。

字段依据 [Desktop Entry 规范](https://specifications.freedesktop.org/desktop-entry/latest/)和 [Icon Theme 规范](https://specifications.freedesktop.org/icon-theme/latest/)。

## 启动

客户端只提交服务端生成的 ID。启动时重新解析对应入口，处理 Exec 字段中的文件/URL、图标、名称、位置和百分号替换，不接受客户端提供的程序路径、argv 或 shell 字符串，不用 shell 求值。没有文件请求时移除文件/URL 字段。缺少程序、工作目录、终端或不支持的启动方式会明确标为不可启动；只有 D-Bus 激活、没有 Exec 兼容入口的程序暂不支持独立启动。

独立会话沿用 Xvfb / Openbox、缩放、H.264、控制端输入法和主动双向文本剪贴板。终端型入口使用 xterm；新建 XTerm/UXTerm 实例添加 Ctrl+Shift+C/V 与 CLIPBOARD 的绑定，不修改全局 Xresources、原有窗口或用户配置。Snap 入口在用户应用 scope 前台运行，使用 Snap 可读目录里的私有会话 cookie，结束时清理；不关闭认证或沙箱。

用户 Agent 服务需使用正常的同 UID 应用启动权限，不能对整棵应用进程树设置 `NoNewPrivileges=yes`：`systemd-run --scope` 会继承该标志，导致 Snap 的 setuid `snap-confine` 在建立自身沙箱前就因缺少 capability 退出。服务模板已移除此标志；没有以 root 启动 Agent，没有关闭 Snap/AppArmor 沙箱，也没有向 QUIC 增加任意命令执行接口。其他本地部署若额外继承了该限制仍需调整。`snap_service_launch_smoke.py` 用同一个真实 Firefox Snap 的 `--version` 在旧限制和当前服务策略下对照，不打开或复制浏览器配置。

HexHub 的 `HexHub.desktop` / `/opt/hexhub/HexHub` 入口自动采用 `desktop_secrets` 兼容策略：每个会话通过独立的 `xdg-dbus-proxy` socket 访问已登录用户的 `org.freedesktop.secrets`，窗口仍位于自己的 Xvfb。不会向应用开放整个桌面总线、其他服务或应用激活；不添加关闭密码加密或沙箱的参数。其他应用默认仍使用 `dbus-run-session`。确实需要同类兼容的本地自定义入口可配置 `"session_bus":"desktop_secrets"`，或在本地 `.desktop` 文件添加 `X-LANRemoteApp-SessionBus=desktop_secrets`；不适用于 Snap 或 existing 窗口。代理随会话清理，Agent 意外退出时也会通过生命周期描述符停止。缺少代理或桌面总线时明确拒绝启动。

发现入口不等于所有应用已完成运行兼容性验收。单实例浏览器、依赖完整 GNOME 会话、GPU 或原生 Wayland 的程序可能需要专门适配；已有窗口可从运行窗口区接入。不会复制默认浏览器资料或改其配置。

应用入口优先连接当前设备上同一应用的已有会话，其次按桌面入口与窗口的关联连接已有桌面窗口；多个候选时要求在运行窗口列表中选择，不任意选择或重复启动。已被控制的会话返回占用提示。恢复会话保留原缩放，不用新启动偏好修改正在运行的程序。

新启动需要观察到可见窗口才返回成功。产生窗口前退出返回 `application_start_failed`；十秒内仍无窗口返回 `application_window_pending`，保留会话和请求 ID，稍后可安全重试。程序退出码为零并不代表窗口已创建（单实例应用转交请求也会正常退出）。

## 窗口与范围

Agent 保存启动时的图形桌面 DISPLAY / Xauthority；环境不可用时，从当前用户的 systemd manager 和已有显式配置定位授权的本地显示。发现只读窗口属性、进程身份和图标，**不截取窗口内容、不改尺寸、不激活窗口、不发送键鼠**。自己的独立虚拟桌面不重复列为原桌面窗口。

X11 窗口经 XRes 确认本地进程并检查同 UID；GNOME 原生 Wayland 窗口由合成器提供窗口身份、PID 和桌面入口 ID，再核验进程启动时间与同 UID。ID 包含进程启动时间和窗口标识，刷新保持稳定，PID 重用不能重定向旧 ID。点击接管时再次验证身份；只接入选中窗口及关联弹窗，同 PID 的其他主窗口仍被排除。旧的自定义 existing 配置沿用整个已批准 X11 进程的范围。

Mac 主窗口关闭只断开连接，远端窗口继续运行；关联预览的关闭按钮只关闭该远端预览，不结束主会话；在远端应用内部关闭选中窗口才会结束对应会话，原进程的其他窗口继续运行；停止 Agent 不结束外部已有进程。运行列表显示连接状态，已连接的窗口须先断开才能再连接。返回启动器时在后台刷新运行状态，同时到达的聚焦事件共用一次读取，连接按钮保持可用；也可点击“刷新运行状态”。这些操作不重读应用目录，较早的状态回包不能覆盖连接后的新状态。重连失败会报告错误并刷新列表，不会自动另开应用。

跨进程关联使用 XRes 的真实客户端 PID、同 UID 的 `/proc` 启动时间与父进程链。显式 `WM_TRANSIENT_FOR` 必须指向本会话已接纳窗口；无该属性的窗口还须与根进程使用相同可执行文件，且在本会话最近输入后新出现或重新显示。过去 5 秒内存在同应用多个会话输入时，不推测无父窗口弹窗的归属。伪造 `_NET_WM_PID`、窗口类或标题不能越过这一检查。同应用多个独立窗口原有的隔离规则保持有效，已经接纳的子窗口在目录中归到原会话。

原生 Wayland 使用 GNOME 50 专用窗口桥接及 PipeWire，不经过 X11 采集。需安装并启用 `paneferry@local` 扩展；首次安装可能需下次登录 GNOME 才识别。目录只按合成器提供的精确 desktop ID 关联，不能匹配时仍可在运行窗口列表直接选择。已知原生 Wayland 程序正在运行而桥接不可用时，返回明确错误，不转发启动到其他显示或创建独立 Firefox 配置。普通 X11/Xwayland 不依赖扩展也可继续工作；GNOME 扩展可进一步处理 Xwayland 的精确尺寸请求。桌面锁屏和系统输入授权仍会影响实际键鼠；读取元数据成功不代表输入已获授权。

Wayland 租约绑定合成器窗口 ID、PID 和本地 D-Bus 连接，断开后释放。关联范围只跟随明确的 transient 链，不借用 X11 的无父窗口推测规则。已接收的远程窗口按进程身份注册排除，不能再发布到第三台设备。Firefox 的窗口接入直接使用原进程，账号、标签页和配置保留在原处；实际 Firefox 验收状态见[分辨率与 Wayland 验证](window-resolution.md)。

原桌面 Xwayland 的系统输入授权集中由 Agent 管理：首次确认并允许记住后，其他应用和断开重连共用同一份授权，服务重启使用系统恢复凭据。列表发现不弹授权窗口，也不放宽所选主窗口/弹窗范围；具体依赖和恢复边界见 [统一输入授权](linux-agent.md#原桌面统一输入授权)。

## 验证

`tests/e2e/app_discovery_smoke.py --agent /ABS/paneferry-agent` 在独立 HOME / XDG / Xvfb 验证桌面入口、本地化、SVG 图标、覆盖和隐藏规则、刷新及真实启动；再以同一进程的三个窗口和按 F2 创建的独立子进程验证主窗口/弹窗隔离、跨进程预览、其他会话与伪造属性输入拒绝、Unicode、主动复制粘贴、重连、子窗口独立关闭及原进程存活。只借用系统公开的图片 loader、主题等配置，不枚举用户原桌面。

实机与构建结果见 [验证记录](validation.md)。

`tests/e2e/session_bus_smoke.py --agent /ABS/paneferry-agent` 使用临时用户总线和伪 Secret Service，验证仅允许目标服务、拒绝其他服务及其 unique name、禁止应用名称注册、多个会话的独立 socket、默认隔离策略、清理和启动失败回滚。测试不访问真实密钥内容。
