# Ubuntu Debian 安装包

`paneferry_0.1.1-1_amd64.deb` 是追加到 v0.1.1 Release 的系统安装包。Debian 修订号为 `1`，产品版本仍为 `0.1.1`。原 `v0.1.1` 标签、Mac ZIP 和 Linux tar 包不覆盖；补充包对应的源码提交由 Release 单独标明。

## 使用与布局

首次安装推荐在 Ubuntu 文件管理器中双击 `.deb`，由应用中心、软件安装或 GDebi 处理。包管理器负责安装依赖；也可用 `sudo apt install ./paneferry_0.1.1-1_amd64.deb`。完整操作、旧脚本版迁移及卸载见[使用教程](usage.md)。

- `/usr/bin/paneferry`：桌面启动入口。
- `/usr/lib/paneferry/`：GTK 界面和配套 Agent / 网络组件；不会优先使用旧用户目录中的二进制。
- `/usr/lib/systemd/user/remoteapp-agent.service`：用户服务，GUI 首次启动时按需启动；没有 systemd 用户管理器时保持直接启动兼容。
- `/usr/share/applications/io.lanremoteapp.launcher.desktop`：应用菜单入口。
- `/usr/share/gnome-shell/extensions/paneferry@local/`：GNOME 扩展，需当前用户启用，首次安装可能要重新登录。
- `/usr/share/doc/paneferry/`：官方许可证、copyright 副本及含在线链接的教程。

包没有 preinst/postinst/prerm/postrm 用户会话操作，也不自动启动系统级 root Agent。安装、升级及卸载不主动停止现有应用，不修改用户配对、配置或家目录。实际使用仍需用户授权本机共享与系统输入。

## 构建与验证命令

在具有 Rust、Python 3.11+、dpkg-dev 和运行依赖的 Ubuntu 上：

```sh
bash scripts/build-deb.sh
python3 tests/e2e/deb_package_check.py dist/paneferry_0.1.1-1_amd64.deb
```

脚本从 Cargo 工作区读取产品版本，默认 Debian 修订号为 `1`；可用 `PANEFERRY_DEB_REVISION` 指定正整数修订号。包架构取当前构建机的 Debian 架构，ELF 依赖通过 `dpkg-shlibdeps` 计算，并显式补充动态加载的图形、输入、Portal 和 GStreamer 组件。本次 amd64 构建要求 `libc6 >= 2.39`；发行版兼容验收仅覆盖 Ubuntu 26.04。

包检查验证依赖、桌面入口、辅助程序路径、用户服务、扩展、文件所有者/权限、LICENSE 内容及不存在会话操作脚本。`linux_runtime_unit.py` 覆盖系统/用户安装路径、执行权限、用户服务启动与无 systemd 时的兼容路径。

## 2026-10-24 已完成的验证

在独立的官方 `ubuntu:26.04` Docker 环境中运行 Xvfb、Openbox、PCManFM 和 GDebi。容器没有映射宿主用户目录或显示服务器；安装器在容器内运行，应用以普通测试用户 UID 1001 启动。

- 文件管理器中双击真实 `.deb` 打开 GDebi，正确显示 `paneferry`、版本 `0.1.1-1` 和缺失依赖。
- 点击“Install Package”，由图形安装器下载依赖并显示“Installation finished”，dpkg 确认安装完成。
- 普通用户从已安装的桌面入口打开窗渡，实际 GTK 界面显示“Ubuntu · 发布与接收已就绪”，Agent 与网络组件从 `/usr/lib/paneferry/` 启动。
- `systemd-analyze verify` 用户服务检查通过；容器内实际启动使用无 systemd 用户管理器时的兼容路径。
- Ubuntu 上的原生 QUIC 路由和 GTK 接收回归通过，覆盖真实图像、键鼠、文本/图片/文件剪贴板、关联窗口、重连和设备隔离。
- 由相同载荷制作 `0.1.1-0` 测试旧包，通过 GDebi 升级到最终 `0.1.1-1`，再图形重装一次；Agent、网络进程 PID、设备身份文件哈希和用户配置哨兵均保持。
- 图形卸载显示“Removal finished”；程序、桌面入口和用户服务文件移除，用户身份与配置保留，既有后台进程未被安装器结束。`apt-get check` 通过。
- `dpkg --verify paneferry` 安装文件校验通过；已安装 LICENSE 的 SHA-256 与仓库官方文本一致。官方容器默认过滤大部分文档，测试中仅为 `/usr/share/doc/paneferry/*` 放开文档保留后重装验证，宿主系统未修改。
- 宿主正式 Agent 在整个测试前后保持同一 PID，测试容器随后清理。

[图形安装完成截图](validation-artifacts/2026-10-24-deb-installed.png) · [普通用户 GUI 就绪截图](validation-artifacts/2026-10-24-deb-manager.png)

这验证了 Ubuntu 中的真实图形安装与启动，但不等同于 GNOME App Center 的完整认证流程、所有桌面环境或全部既有应用的跨机回归。宿主 Ubuntu 正式 Agent 和正在运行的用户应用不参与安装测试。
