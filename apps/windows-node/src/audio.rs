//! An audio subscription exclusively owns its IPC/QUIC control stream. Video
//! never queues here. Sixteen acknowledged packets bound reliable-stream backlog.
use super::*;
#[cfg(any(windows, test))]
use anyhow::Context;
use std::time::Instant;
type Requests = tokio::sync::mpsc::Receiver<Result<(Value, Instant)>>;

#[cfg(windows)]
pub async fn serve<W: AsyncWrite + Unpin>(
    state: &Arc<Mutex<State>>,
    requests: &mut Requests,
    write: &mut W,
) -> Result<()> {
    let result = run(state, requests, write).await;
    if let Err(error) = &result {
        eprintln!("PaneFerry system audio: {error:#}");
        let _ = send(write, json!({"type":"error","code":"audio_unavailable"})).await;
    }
    result
}
#[cfg(not(windows))]
pub async fn serve<W: AsyncWrite + Unpin>(
    _: &Arc<Mutex<State>>,
    _: &mut Requests,
    write: &mut W,
) -> Result<()> {
    send(write, json!({"type":"error","code":"audio_unavailable"})).await
}
async fn send<W: AsyncWrite + Unpin>(write: &mut W, mut value: Value) -> Result<()> {
    value["version"] = json!(1);
    tokio::time::timeout(Duration::from_millis(250), framing::json(write, &value)).await??;
    Ok(())
}
#[cfg(windows)]
async fn run<W: AsyncWrite + Unpin>(
    state: &Arc<Mutex<State>>,
    requests: &mut Requests,
    write: &mut W,
) -> Result<()> {
    let mut audio = tokio::task::spawn_blocking(paneferry_audio::subscribe).await??;
    run_packets(state, requests, write, &mut audio.receiver).await
}
#[cfg(any(windows, test))]
async fn run_packets<W: AsyncWrite + Unpin>(
    state: &Arc<Mutex<State>>,
    requests: &mut Requests,
    write: &mut W,
    audio: &mut tokio::sync::broadcast::Receiver<Result<paneferry_audio::Packet, String>>,
) -> Result<()> {
    send(write, json!({"type":"audio.format","codec":"opus","sample_rate":48000,"channels":2,"frame_samples":480})).await?;
    let start = Instant::now();
    let mut outstanding = std::collections::VecDeque::<(u64, Instant)>::new();
    let mut last_sent = 0;
    let mut last_ack = 0;
    let mut timer = tokio::time::interval(Duration::from_millis(200));
    let mut heartbeat = Instant::now();
    let mut measured = Instant::now();
    let mut received_stats = Instant::now();
    let (mut sent, mut credit_drops, mut age_drops, mut lagged, mut ack_max_us) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    loop {
        tokio::select! {
            biased;
            request = requests.recv() => {
                let (value, _) = request.context("audio disconnected")??;
                ensure!(value["version"] == 1 && value["type"] == "audio.ack", "invalid audio request");
                let ack = value["sequence"].as_u64().context("invalid audio acknowledgment")?;
                ensure!(ack >= last_ack && ack <= last_sent, "invalid audio acknowledgment");
                last_ack = ack;
                while outstanding.front().is_some_and(|(sequence, _)| *sequence <= ack) {
                    if let Some((_, when)) = outstanding.pop_front() { ack_max_us = ack_max_us.max(when.elapsed().as_micros() as u64); }
                }
                // Only bounded counters are accepted, never arbitrary peer log text.
                if received_stats.elapsed() >= Duration::from_secs(4) {
                    if let Some(stats) = value.get("playback").and_then(playback_stats) {
                        eprintln!("PF_AUDIO_RECEIVE {stats}");
                        received_stats = Instant::now();
                    }
                }
            }
            _ = timer.tick() => {
                let sharing = { let state = state.lock().unwrap(); state.sharing && !state.stopping };
                ensure!(sharing, "sharing disabled");
                ensure!(!outstanding.front().is_some_and(|(_, sent)| sent.elapsed() > Duration::from_millis(500)), "audio receiver stalled");
                if measured.elapsed() >= Duration::from_secs(5) {
                    eprintln!("PF_AUDIO_SEND interval_ms={} sent={sent} credit_drops={credit_drops} age_drops={age_drops} lagged={lagged} ack_max_us={ack_max_us} outstanding={}", measured.elapsed().as_millis(), outstanding.len());
                    (sent, credit_drops, age_drops, lagged, ack_max_us) = (0, 0, 0, 0, 0);
                    measured = Instant::now();
                }
                if heartbeat.elapsed() >= Duration::from_secs(1) {
                    send(write, json!({"type":"audio.heartbeat"})).await?;
                    heartbeat = Instant::now();
                }
            }
            packet = audio.recv() => {
                let packet = match packet {
                    Ok(Ok(packet)) => packet,
                    Ok(Err(error)) => bail!("{error}"),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => { lagged += count; continue; },
                    Err(_) => bail!("audio capture ended"),
                };
                if outstanding.len() >= 16 { credit_drops += 1; continue; }
                if packet.created.elapsed() > Duration::from_millis(60) { age_drops += 1; continue; }
                ensure!(state.lock().unwrap().sharing, "sharing disabled");
                send(write, json!({"type":"audio.frame","sequence":packet.sequence,"timestamp_ms":packet.created.saturating_duration_since(start).as_millis() as u64,"data":STANDARD.encode(packet.data)})).await?;
                last_sent = packet.sequence;
                sent += 1;
                outstanding.push_back((last_sent, Instant::now()));
            }
        }
    }
}

#[cfg(any(windows, test))]
fn playback_stats(value: &Value) -> Option<Value> {
    let mut result = serde_json::Map::new();
    for key in [
        "underruns",
        "packet_drops",
        "missing",
        "concealed",
        "pcm_drops",
        "queued_ms",
        "callback_max",
        "sample_rate",
        "prebuffer_ms",
        "stale_packets",
        "arrival_max_ms",
    ] {
        let count = value.get(key)?.as_u64().filter(|v| *v <= 1_000_000_000)?;
        result.insert(key.into(), json!(count));
    }
    Some(Value::Object(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn audio_backlog_is_bounded_and_sharing_off_closes_subscription() {
        let state = State::new("fixture".into(), true);
        let (requests, mut receive) = tokio::sync::mpsc::channel(8);
        let (packets, mut audio) = tokio::sync::broadcast::channel(8);
        let (mut output, input) = tokio::io::duplex(8192);
        let shared = state.clone();
        let task = tokio::spawn(async move {
            run_packets(&shared, &mut receive, &mut output, &mut audio).await
        });
        let mut reader = BufReader::new(input);
        assert_eq!(
            framing::line(&mut reader).await.unwrap()["type"],
            "audio.format"
        );
        for sequence in 1..=16 {
            packets
                .send(Ok(paneferry_audio::Packet {
                    sequence,
                    created: Instant::now(),
                    data: vec![1, 2, 3],
                }))
                .unwrap();
            let packet = framing::line(&mut reader).await.unwrap();
            assert_eq!(packet["sequence"], sequence);
        }
        packets
            .send(Ok(paneferry_audio::Packet {
                sequence: 17,
                created: Instant::now(),
                data: vec![4],
            }))
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), framing::line(&mut reader))
                .await
                .is_err()
        );
        requests
            .send(Ok((
                json!({"version":1,"type":"audio.ack","sequence":16}),
                Instant::now(),
            )))
            .await
            .unwrap();
        packets
            .send(Ok(paneferry_audio::Packet {
                sequence: 18,
                created: Instant::now(),
                data: vec![5],
            }))
            .unwrap();
        assert_eq!(framing::line(&mut reader).await.unwrap()["sequence"], 18);
        state.lock().unwrap().sharing = false;
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("sharing disabled"));
        assert_eq!(packets.receiver_count(), 0);
    }
}
