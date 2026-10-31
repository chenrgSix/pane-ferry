# 有损链路的发送拥塞控制对照

## 实机证据

Mac 更新至 `5539c0e` 后，用户反馈改善但仍卡顿。
回传的 `receiver.feedback_hold` 中位数降至 0–1 µs，前台样本 P95 约 11 ms；
解码约 1.6 ms，显示提交约 0.14 ms，反馈往返仍有约 100 ms 尖峰。
Windows QUIC 统计显示 RTT 约 22–23 ms，同时出现包损失和拥塞事件；
部分两秒区间损失 1–5 包，一次损失 12/833 包，拥塞窗口缩至约 47 KB。
流量控制 `data_blocked` / `stream_data_blocked` 计数为零。
这证明有 QUIC 判定的丢包，不证明具体网卡、隧道或驱动造成丢包。

## 隔离实验

`crates/network/tests/quic_loss_latency.rs` 使用临时身份和本机 UDP 延迟代理，
不访问用户桌面或现有配对。双向各 12 ms；固定 60 Hz、32 KiB/帧，
沿用六帧累计确认上限和一个有序可靠视频流。校验每帧编号和全部测试像素字节。
模拟前向带宽，不将返程 ACK 放进前向排队；每 500 个大 UDP 包丢一个。
Windows 使用与活动采集相同的 1 ms 定时精度，并在退出时恢复。

本机一次 360 帧对照（前 60 帧不计延迟分位数）：

| 链路 | 发送控制 | 帧交付延迟 P50 / P95 | 总耗时 | 等待额度的计时次数 |
| --- | --- | --- | --- | --- |
| 100 Mbps，无丢包 | CUBIC | 16.3 / 18.0 ms | 6.02 s | 0 |
| 同上 | BBR | 15.4 / 16.3 ms | 6.01 s | 0 |
| 20 Mbps，约 0.2% 丢包 | CUBIC | 144 / 170 ms | 9.06 s | 175 |
| 同上 | BBR | 26.5 / 44.5 ms | 6.02 s | 0 |
| 10 Mbps，无丢包，带宽低于源速率 | CUBIC | 139 / 148 ms | 9.77 s | 218 |
| 同上 | BBR | 140 / 147 ms | 9.74 s | 216 |

这不是用户网络的端到端结果。不同机器/定时精度会改变数值。
丢包后的可靠流仍须重传，BBR 也有约 52 ms 最大交付间隔；它不能消除所有卡顿。
先前试验将 QUIC ACK 延迟缩至 5 ms 未看到同量级改善，未修改生产 ACK 参数。

运行：

```sh
cargo test --release --locked -p paneferry-network --test quic_loss_latency -- --ignored --nocapture
```

## 可回退试用

新增进程环境变量 `PANEFERRY_QUIC_CONGESTION=bbr`，在启动时选用 Quinn 的 BBR。
默认和显式 `cubic` 均保留 CUBIC，未知值记录提示后仍使用 CUBIC。
Quinn 将其 BBR 标为实验性，因此先在这台 Windows 上对照，不全局替换默认配置。
参考 [Quinn Bbr 文档](https://docs.rs/quinn/latest/quinn/congestion/struct.Bbr.html)。

仅控制本机 QUIC 发送；不改变协议、认证、编码、帧队列、路由或 GPU 要求，
Mac 维持 `5539c0e` 即可。连接共享该拥塞控制，影响同连接的所有发送流，
控制流原有优先级不变。回退时清除变量或改为 `cubic` 并重启发送端。
本次部署仅为发布端/启动器进程设置环境变量，不写系统环境或用户配对配置。

启用 BBR 的 Windows 完整检查及真实配对 QUIC 回归通过；
是否改善这条实际链路的剩余卡顿，仍需相同应用、相同画面设置的用户复测。
