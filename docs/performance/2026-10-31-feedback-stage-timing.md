# 周期性卡顿的接收端计时

`6d9876c` 部署后的实际复现不再出现频繁 `geometry_race` 新流，但用户仍感到周期性卡顿。稳定采集编码约 3 ms，帧反馈往返出现 150–250 ms 尖峰，实际画布尺寸变化仍会重建编码器。不能把所有剩余延迟归因于网络，也不能把去掉重复开流等同于端到端问题已解决。

本轮增加诊断，不改变六帧在途限制、50 ms 反馈合并周期、画质调节和编码策略。

Windows hello 广告 `video.feedback.timing.v1`。仅当控制端看到此能力，且拥有对应帧完整的本机单调时间戳时，`video.feedback` 才带可选 `timing` 对象：

| 字段 | 范围 |
| --- | --- |
| `queue_us` | 接收端读入帧到开始解码 |
| `decode_us` | 解码、色彩还原到解码任务完成 |
| `present_us` | 解码完成到显示提交回调，包括主线程排队 |
| `feedback_wait_us` | 显示提交到发送累计反馈，包括反馈合并等待 |

字段均为微秒整数，最大 5 秒；不传绝对时间、窗口标题、输入内容或像素。提交回调不是显示器实际扫描输出，不将它称为真实上屏时间。

旧发布端仍只收到 `frame_id`；旧控制端仍可不传计时。报告只用于日志，不改变发送额度或质量算法。未知/过期帧的报告不起作用，超界/未知字段被拒绝，总耗时大于本地观测往返的报告不纳入统计。

启用 Windows `PANEFERRY_VIDEO_TIMING=1` 后，在原计时日志中新增：

- `receiver.decode_queue`
- `receiver.decode`
- `receiver.decode_to_commit`
- `receiver.feedback_hold`
- `publisher.transport_and_bridge_roundtrip`：总往返减去上述接收端区间，仍包括网络、IPC/bridge 和反馈返回，不能当成纯网络 RTT。
- `publisher.feedback_gate_wait`：在途限制触发后暂停采集的时长。

Windows 测试覆盖可选字段、类型和边界、未知帧不能提供发送额度、真实配对 QUIC 接受新反馈。Mac 原生测试覆盖计时计算、缺失/逆序/超界时间、能力协商和测量必须匹配累计确认的帧编号。下一步需要更新 Mac 控制端并复现，让这些分段信息自动回传到 Windows。
