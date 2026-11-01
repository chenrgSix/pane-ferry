//! Audio owns a separate authenticated route. Video never enters this queue.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use std::collections::VecDeque;

pub fn serve(
    state: &Arc<Mutex<State>>,
    requests: &mpsc::Receiver<Request>,
    output: &mut output::Output,
) -> Result<()> {
    let result = run(state, requests, output);
    if let Err(error) = &result {
        eprintln!("PaneFerry system audio: {error:#}");
        let _ = send(
            output,
            json!({"type":"error","code":"audio_unavailable"}),
            "",
        );
    }
    result
}
fn run(
    state: &Arc<Mutex<State>>,
    requests: &mpsc::Receiver<Request>,
    output: &mut output::Output,
) -> Result<()> {
    anyhow::ensure!(state.lock().unwrap().sharing, "sharing disabled");
    let mut audio = paneferry_audio::subscribe()?;
    send(
        output,
        json!({"type":"audio.format","codec":"opus","sample_rate":48000,"channels":2,"frame_samples":480}),
        "",
    )?;
    let start = Instant::now();
    let mut heartbeat = start;
    let mut measured = start;
    let mut outstanding = VecDeque::<(u64, Instant)>::new();
    let (mut last_sent, mut last_ack) = (0, 0);
    let (mut sent, mut credit_drops, mut age_drops, mut lagged, mut ack_max_us) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    while !STOP.load(Ordering::Acquire) {
        anyhow::ensure!(state.lock().unwrap().sharing, "sharing disabled");
        anyhow::ensure!(
            !outstanding
                .front()
                .is_some_and(|(_, when)| when.elapsed() > Duration::from_millis(500)),
            "audio receiver stalled"
        );
        match requests.recv_timeout(Duration::from_millis(2)) {
            Ok(Request::AudioAck {
                sequence,
                playback: _,
            }) => {
                anyhow::ensure!(
                    sequence >= last_ack && sequence <= last_sent,
                    "invalid audio acknowledgment"
                );
                last_ack = sequence;
                while outstanding.front().is_some_and(|(seq, _)| *seq <= sequence) {
                    ack_max_us = ack_max_us
                        .max(outstanding.pop_front().unwrap().1.elapsed().as_micros() as u64);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
            _ => bail!("invalid audio request"),
        }
        loop {
            let packet = match audio.receiver.try_recv() {
                Ok(Ok(packet)) => packet,
                Ok(Err(error)) => bail!("{error}"),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                    lagged += n;
                    continue;
                }
                Err(_) => bail!("audio capture ended"),
            };
            if outstanding.len() >= 16 {
                credit_drops += 1;
                continue;
            }
            if packet.created.elapsed() > Duration::from_millis(60) {
                age_drops += 1;
                continue;
            }
            anyhow::ensure!(state.lock().unwrap().sharing, "sharing disabled");
            send(
                output,
                json!({"type":"audio.frame","sequence":packet.sequence,"timestamp_ms":packet.created.saturating_duration_since(start).as_millis() as u64,"data":STANDARD.encode(&packet.data)}),
                "",
            )?;
            last_sent = packet.sequence;
            sent += 1;
            outstanding.push_back((last_sent, Instant::now()));
        }
        if heartbeat.elapsed() >= Duration::from_secs(1) {
            send(output, json!({"type":"audio.heartbeat"}), "")?;
            heartbeat = Instant::now();
        }
        if measured.elapsed() >= Duration::from_secs(5) {
            eprintln!("PF_AUDIO_SEND sent={sent} credit_drops={credit_drops} age_drops={age_drops} lagged={lagged} ack_max_us={ack_max_us} outstanding={}",outstanding.len());
            (sent, credit_drops, age_drops, lagged, ack_max_us) = (0, 0, 0, 0, 0);
            measured = Instant::now();
        }
    }
    Ok(())
}

/// GTK delegates Opus/PLC/jitter to the same player as the native launchers.
/// Only local stdin is accepted; it never exposes a playback socket.
pub fn play() -> Result<()> {
    let player = paneferry_audio::Player::start()?;
    let mut output = std::io::stdout().lock();
    writeln!(output, "{{\"type\":\"audio.ready\"}}")?;
    output.flush()?;
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut last = 0;
    let mut report = Instant::now();
    while let Some(line) = read_line_limit(&mut input, 8192)? {
        let event: Value = serde_json::from_slice(&line)?;
        let sequence = event["sequence"]
            .as_u64()
            .context("invalid audio sequence")?;
        anyhow::ensure!(
            event["type"] == "audio.frame" && sequence > last,
            "invalid audio sequence"
        );
        let data = STANDARD.decode(event["data"].as_str().context("invalid audio packet")?)?;
        player.push(paneferry_audio::Packet {
            sequence,
            created: Instant::now(),
            data,
        })?;
        last = sequence;
        let mut reply = json!({"type":"audio.ack","sequence":sequence});
        if report.elapsed() >= Duration::from_secs(5) {
            let stats = player.stats();
            reply["playback"] = json!({
            "underruns":stats.underruns,"packet_drops":stats.packet_drops,"missing":stats.missing,
            "concealed":stats.concealed,"pcm_drops":stats.pcm_drops,"queued_ms":stats.queued_ms,
            "callback_max":stats.callback_max,"sample_rate":stats.sample_rate,"prebuffer_ms":stats.prebuffer_ms});
            report = Instant::now();
        }
        writeln!(output, "{reply}")?;
        output.flush()?;
    }
    Ok(())
}
