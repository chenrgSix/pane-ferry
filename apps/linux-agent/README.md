# remoteapp-agent

完整安装、运行和限制说明见 [Linux Agent 指南](../../docs/linux-agent.md)。

在仓库根目录构建：

```sh
cargo build --release -p remoteapp-agent
mkdir -p "$HOME/.local/bin" "$HOME/.config/lan-remoteapp"
install -m 755 target/release/remoteapp-agent "$HOME/.local/bin/remoteapp-agent"
# 审核示例内路径和 argv 后，再自行创建 ~/.config/lan-remoteapp/apps.json。
~/.local/bin/remoteapp-agent check
~/.local/bin/remoteapp-agent serve
```

以上安装命令供你在自己的 Linux 上审阅执行，本次交付没有执行远端安装。不要覆盖已有白名单；仅运行你认可的应用。
