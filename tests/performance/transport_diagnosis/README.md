# 2026-10-22 双机传输诊断夹具

> 历史实验复现：SSH/QUIC 对照基于 `c927116`（移除 SSH 前）的代码与夹具。请在该提交的独立检出中运行下列命令；当前产品已移除 SSH 适配器和密码助手，不再满足这些历史源码插桩位置。原始数据与结果保持不变。

这是本次实验使用的固定环境脚本，不是自动部署或通用压测工具。脚本中的地址、用户名、接口、临时目录均对应报告环境；换机器前必须逐项替换。它们不会修改产品源文件或替换已安装程序。EasyTier 开关由操作者单独修改、核实、恢复。

## 环境和范围

- 仓库：`/Users/chenzhirong/Code/ideProject/lan-remoteapp`。
- Mac 临时目录：`/private/tmp/lra-path-diagnosis`，Linux：`/tmp/lra-path-diagnosis`。
- Mac：物理 `192.168.1.80/en0`、虚拟 `10.0.1.253/utun4`。
- Ubuntu：物理 `192.168.1.26/enp1s0`、虚拟 `10.0.1.66/tun0`，用户 `chenrg`。
- 使用隔离的 QUIC 身份、配对、发布端和文件，不读产品目录、桌面、剪贴板或用户文件。
- 文件是 128 MiB 随机字节；SSH 与原生 QUIC 使用同一 C 文件端点。QUIC 经过产品的 `bulk-upload` / `bulk-download`、本地 Unix socket 和可靠流。
- 这能比较传输和 bulk 桥接，不是完整剪贴板文件格式或桌面视频 E2E。
- 接收完成计时包含 `fsync`，计时后独立验证 SHA-256。Ubuntu `/tmp` 是 tmpfs；不代表 Ubuntu NVMe 持久化速度。

## 准备

仅在没有同名实验运行时使用此目录。先保存需要保留的旧实验结果，不要对未知目录执行清理。

1. 创建两端测试目录（权限 0700），将这里的 `.py` 和 `.c` 文件复制到对应目录。Mac 的 `ssh/main.swift` 复制为临时目录中的 `main.swift`。
2. `ssh/main.swift` 通过项目既有 `SSHAuthentication.swift` 使用启动器保存的凭据；不输出密码，不把密码放入命令行。物理地址强制使用虚拟地址的已知主机密钥别名，目标必须是同一设备。若不使用此认证方式，用等价的 SSH 包装器替换临时目录中的 `ssh`：接口为 `ssh --target ADDRESS COMMAND`，必须保留主机密钥校验、无压缩和独立的持久连接。

```sh
swiftc -O -module-cache-path /private/tmp/lra-path-diagnosis/swift-cache \
  apps/mac-windowhost/Sources/RemoteAppWindowHost/SSHAuthentication.swift \
  /private/tmp/lra-path-diagnosis/main.swift \
  -o /private/tmp/lra-path-diagnosis/ssh
```

3. 两端使用 ESnet 的 `iperf-3.22.tar.gz`，来源：
   <https://github.com/esnet/iperf/releases/download/3.22/iperf-3.22.tar.gz>。
   SHA-256：`1c0d0fb02c52626111d6e132db80edfbf27bbaff8bd9245df2a371dcb0b35a92`。
   解压到测试目录后运行 `./configure --without-openssl --disable-shared`、`make -j4`，使用 `iperf-3.22/src/iperf3`。不要全局安装。
4. Mac 运行 `prepare.py`。它复制 Cargo 工作区到临时目录，只给临时 network 源码增加连接统计。把临时 `source` 目录传到 Ubuntu。两端分别执行：

```sh
# Mac 使用 /private/tmp 前缀；Linux 使用 /tmp 前缀。
CARGO_TARGET_DIR=/tmp/lra-path-diagnosis/target cargo build \
  --release --offline --locked \
  --manifest-path /tmp/lra-path-diagnosis/source/Cargo.toml \
  -p remoteapp-network
cc -O2 /tmp/lra-path-diagnosis/bench-file.c \
  -o /tmp/lra-path-diagnosis/bench-file
```

5. 生成一份 134217728 字节的随机 `payload.bin`，复制到另一端并核实 SHA-256 相同。重复运行时始终用同一文件；报告保留本次原始文件的哈希。

## 执行顺序

每次只运行一个性能脚本，不要同时构建程序或传输结果。资源采样在脚本内部并行运行，不另外发起吞吐测试。

```sh
python3 /private/tmp/lra-path-diagnosis/run-iperf.py
python3 /private/tmp/lra-path-diagnosis/run-files.py proxy-on
# 只关闭 Mac 当前 EasyTier 网络的“启用 QUIC 代理”，重新核实 P2P 路由。
# 关闭本实验的虚拟地址 SSH master，使新文件会话使用修改后的路径。
python3 /private/tmp/lra-path-diagnosis/run-iperf-off.py
python3 /private/tmp/lra-path-diagnosis/run-udp-overlay.py
python3 /private/tmp/lra-path-diagnosis/run-udp-bound.py
python3 /private/tmp/lra-path-diagnosis/run-files.py proxy-off overlay
# 恢复原 EasyTier 开关，并核实网络及 SSH 可用。
python3 /private/tmp/lra-path-diagnosis/analyze.py
```

- TCP：`-P 1 -t 30`，两个数据方向，各三次；使用接收端字节数和时间。
- UDP：`-P 1 -u -b 100M -l 1200 -t 10`，只作为链路诊断。Ubuntu 同一网卡有两个 IP，物理 UDP 服务必须 `-B 192.168.1.26`，否则通配绑定的回包源地址可能不同，测试无法建立。不要把这种错误记作 0 MB/s。
- 文件每方向、每协议三次，顺序为 SSH→QUIC、QUIC→SSH、SSH→QUIC。连接预先建立；计时包含每次 CLI 启动和会话打开，不包含初次认证/配对。
- 文件正确性断言失败或超时会中止该文件测试。iperf JSON 有 `error` 的样本无效，分析器会排除；必须解决原因并补足三次有效样本。
- `monitor.py` 每秒采集 Linux `/proc` 线程 CPU / UDP / 磁盘 / 网卡计数；Mac 使用 `ps -M`、`netstat`、`iostat`。后续补充了 Mac 整机 CPU ticks 和 `ioreg` 磁盘读写字节；早期样本没有这些字段。
- QUIC 诊断按 `remote` 地址匹配连接，不能随便取数组最后一项：先测过的物理连接可能仍保留统计。
- Mac `ps -M` 的 CPU 是内核采样值，Linux CPU 是相邻 `/proc` ticks 的一秒差分，100% 均表示一个逻辑核。短文件传输的峰值分辨率有限。
- 网卡、UDP、磁盘计数是整机计数，包含背景流量。`SndbufErrors` 也可能包含非阻塞发送遇到缓冲区背压，不能直接等同于不可恢复丢包。

## 单因素接收缓冲区实验

只有在原始矩阵完成、EasyTier 已恢复原设置且没有测试正在运行时执行：

```sh
python3 /private/tmp/lra-path-diagnosis/receive-buffer.py
python3 /private/tmp/lra-path-diagnosis/run-files.py rcvbuf4m physical
```

`receive-buffer.py` 只修改 Mac 临时源副本的 UDP socket `SO_RCVBUF` 为 4 MiB；不改 Linux 二进制、流控窗口、拥塞算法、MTU 或系统 sysctl。它保存原二进制为 `network-baseline`、原文件为 `service-before-buffer.rs`，并通过 `getsockopt` 输出实际生效的字节数。实验后恢复这两个临时文件，或随实验目录一并清理。不要将这个实验自动视为产品性能优化已经验证通过：必须同时检查丢包与并排 SSH 对照的速度。

## 结果与清理

`results/*.json` 是逐次结果，`*-mac.jsonl` / `*-ubuntu.jsonl` 是同步资源采样。`summary.json` 是汇总；保留原始数据才能复核。不要保存或发布 `*-state` 中的私钥、邀请令牌、完整 SSH 调试日志或无关进程的参数。

脚本的 `finally` 终止本次创建的发布端、network 服务及 iperf3 服务。异常退出后按精确 PID/命令路径核对残留，仅清理本实验进程。用 `ssh -S <本实验的 socket> -O exit <目标>` 关闭实验自己的 master。保存报告后删除两端实验身份和临时文件；不修改已安装应用、用户 SSH 连接或原有共享开关。
