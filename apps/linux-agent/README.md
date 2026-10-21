# remoteapp-agent

完整安装、运行和限制说明见 [Linux Agent 指南](../../docs/linux-agent.md)。

在仓库根目录构建：

```sh
cargo build --release -p remoteapp-agent
mkdir -p "$HOME/.local/bin" "$HOME/.config/lan-remoteapp"
install -m 755 target/release/remoteapp-agent "$HOME/.local/bin/remoteapp-agent"
# 已安装桌面应用会自动发现；有固定参数需求时，可另配 apps.json 自定义入口。
~/.local/bin/remoteapp-agent check
~/.local/bin/remoteapp-agent serve
```

安装时保留已有自定义入口。应用名称、图标与运行窗口的发现方式见 [应用与窗口发现](../../docs/application-discovery.md)。
