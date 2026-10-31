//! Opt-in, loopback-only transport experiment. No desktop, pairing or user data.
//! cargo test -p paneferry-network --test quic_loss_latency -- --ignored --nocapture
use paneferry_network::identity::Identity;
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UdpSocket,
    time::Instant,
};

// Match the active Windows capture runtime's timer precision. Otherwise the
// fixture's 12 ms hop and 60 Hz producer are rounded to ~16 ms scheduler ticks.
#[cfg(windows)]
struct TimerResolution;
#[cfg(windows)]
#[link(name = "winmm")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}
#[cfg(windows)]
impl Drop for TimerResolution {
    fn drop(&mut self) {
        unsafe {
            timeEndPeriod(1);
        }
    }
}

fn transport(ack_ms: Option<u64>, bbr: bool) -> Arc<quinn::TransportConfig> {
    let mut config = quinn::TransportConfig::default();
    config
        .mtu_discovery_config(None)
        .stream_receive_window((4u32 * 1024 * 1024).into())
        .receive_window((16u32 * 1024 * 1024).into())
        .send_window(16 * 1024 * 1024);
    if let Some(ms) = ack_ms {
        let mut ack = quinn::AckFrequencyConfig::default();
        ack.max_ack_delay(Some(Duration::from_millis(ms)))
            .reordering_threshold(1u32.into());
        config.ack_frequency_config(Some(ack));
    }
    if bbr {
        config.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    }
    Arc::new(config)
}

async fn run(ack_ms: Option<u64>, bbr: bool, drop_every: usize, mbps: u64) {
    let root = tempfile::tempdir().unwrap();
    let server_id = Identity::load(&root.path().join("server"), None).unwrap();
    let client_id = Identity::load(&root.path().join("client"), None).unwrap();
    let mut config = server_id.server().unwrap();
    config.transport_config(transport(ack_ms, bbr));
    let server = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let server_addr = server.local_addr().unwrap();
    let proxy = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy.local_addr().unwrap();
    // One ordered delay queue; never reorder packets or spawn a task per packet.
    let forwarding = tokio::spawn(async move {
        let mut destination = None;
        let mut queue = VecDeque::new();
        let mut bytes = vec![0; 65536];
        let mut eligible = 0usize;
        let mut wire_free = Instant::now();
        loop {
            let deadline = queue
                .front()
                .map(|(at, _, _)| *at)
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(60));
            tokio::select! {
                result = proxy.recv_from(&mut bytes) => {
                    let (len, source) = result.unwrap();
                    let target = if source == server_addr { destination.unwrap() }
                        else { destination = Some(source); server_addr };
                    let now = Instant::now();
                    let departure = if source == server_addr && mbps > 0 {
                        wire_free = wire_free.max(now) + Duration::from_nanos(len as u64 * 8000 / mbps);
                        wire_free
                    } else { now };
                    if source == server_addr && len >= 1000 {
                        eligible += 1;
                        if drop_every != 0 && eligible % drop_every == 0 {
                            continue;
                        }
                    }
                    assert!(queue.len() < 8192, "fixture delay queue overflow");
                    let at = departure + Duration::from_millis(12);
                    // Reverse ACKs do not wait in the shaped forward queue.
                    let position = queue.partition_point(|(due, _, _)| *due <= at);
                    queue.insert(position, (at, target, bytes[..len].to_vec()));
                }
                _ = tokio::time::sleep_until(deadline), if !queue.is_empty() => {
                    // Drain all due packets in this wakeup. A fresh Windows
                    // timer per packet would itself impose a bandwidth cap.
                    while queue.front().is_some_and(|(at, _, _)| *at <= Instant::now()) {
                        let (_, target, packet) = queue.pop_front().unwrap();
                        proxy.send_to(&packet, target).await.unwrap();
                    }
                }
            }
        }
    });
    let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    let mut config = client_id.client(&server_id.certificate).unwrap();
    config.transport_config(transport(ack_ms, bbr));
    client.set_default_client_config(config);
    let accepting = async { server.accept().await.unwrap().await.unwrap() };
    let dialing = client.connect(proxy_addr, "lan-remoteapp").unwrap();
    let (publisher, receiver) = tokio::join!(accepting, dialing);
    let receiver = receiver.unwrap();
    let publisher_stats = publisher.clone();
    let origin = Instant::now();
    let frames = 360u64;
    let frame_bytes = 32768usize;
    let sending = tokio::spawn(async move {
        let (mut send, mut feedback) = publisher.open_bi().await.unwrap();
        let (ack_tx, mut ack_rx) = tokio::sync::mpsc::channel(6);
        let reading = tokio::spawn(async move {
            for _ in 0..frames {
                ack_tx
                    .send(feedback.read_u64().await.unwrap())
                    .await
                    .unwrap();
            }
        });
        let mut pending = VecDeque::new();
        let mut timer = tokio::time::interval(Duration::from_secs_f64(1.0 / 60.0));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut sequence = 0;
        let mut gated_ticks = 0u64;
        while sequence < frames || !pending.is_empty() {
            tokio::select! {
                ack = ack_rx.recv(), if !pending.is_empty() => {
                    let ack = ack.unwrap();
                    while pending.front().is_some_and(|id| *id <= ack) { pending.pop_front(); }
                }
                _ = timer.tick(), if sequence < frames => {
                    if pending.len() >= 6 { gated_ticks += 1; continue; }
                    let mut frame = vec![0x5a; frame_bytes];
                    frame[..8].copy_from_slice(&sequence.to_be_bytes());
                    frame[8..16].copy_from_slice(&(origin.elapsed().as_micros() as u64).to_be_bytes());
                    send.write_all(&frame).await.unwrap();
                    pending.push_back(sequence);
                    sequence += 1;
                }
            }
        }
        reading.await.unwrap();
        send.finish().unwrap();
        gated_ticks
    });
    let (mut ack, mut read) = receiver.accept_bi().await.unwrap();
    let mut latencies = Vec::new();
    let mut gaps = Vec::new();
    let mut last = None;
    for expected in 0..frames {
        let mut bytes = vec![0; frame_bytes];
        read.read_exact(&mut bytes).await.unwrap();
        let sequence = u64::from_be_bytes(bytes[..8].try_into().unwrap());
        assert_eq!(sequence, expected, "lost/duplicated/reordered frame");
        assert!(bytes[16..].iter().all(|b| *b == 0x5a), "payload corruption");
        let sent = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
        let now = origin.elapsed().as_micros() as u64;
        if expected >= 60 {
            latencies.push(now - sent);
            if let Some(previous) = last {
                gaps.push(now - previous);
            }
        }
        last = Some(now);
        ack.write_u64(sequence).await.unwrap();
    }
    let gated = sending.await.unwrap();
    latencies.sort_unstable();
    gaps.sort_unstable();
    let stats = publisher_stats.stats();
    if drop_every != 0 {
        assert!(
            stats.path.lost_packets > 0,
            "loss fixture did not exercise recovery"
        );
    }
    let p =
        |values: &[u64], percentile: usize| values[(values.len() * percentile).div_ceil(100) - 1];
    assert!(
        *latencies.last().unwrap() < 1_000_000,
        "six-frame credit must bound queue latency in this fixture"
    );
    println!(
        "LOSS_PROBE {}",
        serde_json::json!({"ack_ms":ack_ms,"bbr":bbr,"drop_every":drop_every,
        "frames":frames,"mbps":mbps,"elapsed_ms":origin.elapsed().as_millis(),"gated_ticks":gated,
        "delay_p50_us":p(&latencies,50),"delay_p95_us":p(&latencies,95),"delay_p99_us":p(&latencies,99),
        "gap_p95_us":p(&gaps,95),"gap_p99_us":p(&gaps,99),"gap_max_us":gaps.last(),
        "lost_packets":stats.path.lost_packets,"congestion_events":stats.path.congestion_events,
        "ack_frequency_sent":stats.frame_tx.ack_frequency})
    );
    receiver.close(0u32.into(), b"fixture complete");
    publisher_stats.close(0u32.into(), b"fixture complete");
    forwarding.abort();
    let _ = forwarding.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "loopback RTT/loss/bandwidth comparison; about one minute"]
async fn compare_reliable_video_loss_recovery() {
    #[cfg(windows)]
    let _timer = {
        assert_eq!(unsafe { timeBeginPeriod(1) }, 0);
        TimerResolution
    };
    tokio::time::timeout(Duration::from_secs(120), async {
        for (loss, mbps) in [(0, 100), (500, 20), (0, 10)] {
            for bbr in [false, true] {
                run(None, bbr, loss, mbps).await;
            }
        }
    })
    .await
    .unwrap();
}
