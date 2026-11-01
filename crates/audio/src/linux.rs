//! Pulse/PipeWire output-monitor audio. Helpers are private children, bounded
//! pipes carry PCM; Opus and loss/jitter handling use the shared Rust code.
use crate::{Packet, MAX_PACKET, RATE, SAMPLES};
use anyhow::{ensure, Context, Result};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock, Weak,
    },
    time::{Duration, Instant},
};

pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        ["gst-launch-1.0", "pactl"].iter().all(|binary| {
            Command::new(binary)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    })
}

struct Process(Mutex<Option<Child>>);
impl Process {
    fn stop(&self) {
        if let Some(mut child) = self.0.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn running(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

/// No shell interpolation. Limit both execution time and introspection output.
fn pulse(args: &[&str]) -> Result<Vec<u8>> {
    let mut child = Command::new("pactl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(512 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = send.send(result);
    });
    let result = receive.recv_timeout(Duration::from_secs(2));
    let _ = child.kill();
    let status = child.wait()?;
    let bytes = result.context("audio server unavailable")??;
    ensure!(
        status.success() && bytes.len() <= 512 * 1024,
        "audio server unavailable"
    );
    Ok(bytes)
}
fn default_sink() -> Result<String> {
    let name = String::from_utf8(pulse(&["get-default-sink"])?)?
        .trim()
        .to_owned();
    ensure!(
        !name.is_empty() && name.len() <= 1024,
        "no default audio output"
    );
    Ok(name)
}
fn monitor() -> Result<(String, String)> {
    let name = default_sink()?;
    let sinks: serde_json::Value =
        serde_json::from_slice(&pulse(&["-f", "json", "list", "sinks"])?)?;
    let sink = sinks
        .as_array()
        .context("invalid audio sinks")?
        .iter()
        .find(|s| s["name"].as_str() == Some(&name))
        .context("default audio output unavailable")?;
    let source = sink["monitor_source"]
        .as_str()
        .context("output monitor unavailable")?;
    // Never substitute the default input: it may be a microphone.
    ensure!(
        !source.is_empty()
            && source.len() <= 1024
            && source
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c)),
        "output monitor unavailable"
    );
    Ok((name, source.into()))
}
fn small_pipe(fd: i32) {
    unsafe {
        libc::fcntl(fd, libc::F_SETPIPE_SZ, 4096);
    }
}

struct Capture {
    sender: tokio::sync::broadcast::Sender<Result<Packet, String>>,
    stop: Arc<AtomicBool>,
    process: Arc<Process>,
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.process.stop();
    }
}
pub struct Subscription {
    pub receiver: tokio::sync::broadcast::Receiver<Result<Packet, String>>,
    _capture: Arc<Capture>,
}
pub fn subscribe() -> Result<Subscription> {
    static CAPTURE: OnceLock<Mutex<Weak<Capture>>> = OnceLock::new();
    let mut shared = CAPTURE
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .unwrap();
    if let Some(capture) = shared.upgrade().filter(|c| !c.stop.load(Ordering::Acquire)) {
        return Ok(Subscription {
            receiver: capture.sender.subscribe(),
            _capture: capture,
        });
    }
    let (sink, source) = monitor()?;
    let mut child = Command::new("gst-launch-1.0")
        .args(["-q", "pulsesrc"])
        .arg(format!("device={source}"))
        .args([
            "buffer-time=40000",
            "latency-time=10000",
            "blocksize=3840",
            "!",
            "audioconvert",
            "!",
            "audioresample",
            "!",
            "audio/x-raw,format=F32LE,rate=48000,channels=2,layout=interleaved",
            "!",
            "queue",
            "max-size-buffers=2",
            "max-size-bytes=0",
            "max-size-time=0",
            "leaky=downstream",
            "!",
            "fdsink",
            "fd=1",
            "sync=false",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    small_pipe(stdout.as_raw_fd());
    let process = Arc::new(Process(Mutex::new(Some(child))));
    let stop = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = tokio::sync::broadcast::channel(16);
    let capture = Arc::new(Capture {
        sender: sender.clone(),
        stop: stop.clone(),
        process: process.clone(),
    });
    let (ready, wait) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("system-output-audio".into())
        .spawn(move || {
            let result = (|| -> Result<()> {
                let mut codec = crate::encoder()?;
                let mut raw = [0u8; SAMPLES * 2 * 4];
                let mut pcm = [0.0f32; SAMPLES * 2];
                let mut encoded = [0; MAX_PACKET];
                let mut sequence = 0;
                let mut checked = Instant::now();
                let mut measured = Instant::now();
                let (mut packets, mut max_us) = (0u64, 0u64);
                while !stop.load(Ordering::Acquire) {
                    stdout
                        .read_exact(&mut raw)
                        .context("output monitor ended")?;
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let created = Instant::now();
                    for (out, bytes) in pcm.iter_mut().zip(raw.chunks_exact(4)) {
                        let v = f32::from_le_bytes(bytes.try_into().unwrap());
                        *out = if v.is_finite() {
                            v.clamp(-1.0, 1.0)
                        } else {
                            0.0
                        };
                    }
                    let count = codec.encode_float(&pcm, &mut encoded)?;
                    max_us = max_us.max(created.elapsed().as_micros() as u64);
                    packets += 1;
                    sequence += 1;
                    let _ = sender.send(Ok(Packet {
                        sequence,
                        created,
                        data: encoded[..count].to_vec(),
                    }));
                    if sequence == 1 {
                        let _ = ready.send(Ok(()));
                    }
                    if checked.elapsed() >= Duration::from_secs(2) {
                        ensure!(default_sink()? == sink, "default audio output changed");
                        checked = Instant::now();
                    }
                    if measured.elapsed() >= Duration::from_secs(5) {
                        eprintln!("PF_AUDIO_CAPTURE packets={packets} encode_max_us={max_us}");
                        packets = 0;
                        max_us = 0;
                        measured = Instant::now();
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = ready.try_send(Err(error.to_string()));
                let _ = sender.send(Err(error.to_string()));
            }
            stop.store(true, Ordering::Release);
            process.stop();
        })?;
    wait.recv_timeout(Duration::from_secs(5))?
        .map_err(anyhow::Error::msg)?;
    *shared = Arc::downgrade(&capture);
    Ok(Subscription {
        receiver,
        _capture: capture,
    })
}

#[derive(Default)]
pub struct PlaybackSnapshot {
    pub underruns: u64,
    pub packet_drops: u64,
    pub missing: u64,
    pub concealed: u64,
    pub pcm_drops: u64,
    pub queued_ms: u64,
    pub callback_max: u64,
    pub sample_rate: u64,
    pub prebuffer_ms: u64,
}
pub struct Player {
    packets: Arc<Mutex<VecDeque<Packet>>>,
    stats: Arc<Mutex<PlaybackSnapshot>>,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    process: Arc<Process>,
}
impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.process.stop();
    }
}
impl Player {
    pub fn start() -> Result<Self> {
        // Explicit Pulse output; ALSA/default-input fallback is never used.
        default_sink()?;
        let mut child = Command::new("gst-launch-1.0")
            .args([
                "-q",
                "fdsrc",
                "fd=0",
                "blocksize=3840",
                "!",
                "rawaudioparse",
                "format=pcm",
                "pcm-format=f32le",
                "sample-rate=48000",
                "num-channels=2",
                "!",
                "audioconvert",
                "!",
                "audioresample",
                "!",
                "pulsesink",
                "buffer-time=40000",
                "latency-time=10000",
                "sync=false",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let mut input = child.stdin.take().unwrap();
        small_pipe(input.as_raw_fd());
        let process = Arc::new(Process(Mutex::new(Some(child))));
        let packets = Arc::new(Mutex::new(VecDeque::<Packet>::new()));
        let stats = Arc::new(Mutex::new(PlaybackSnapshot {
            sample_rate: RATE as u64,
            prebuffer_ms: 40,
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let player = Self {
            packets: packets.clone(),
            stats: stats.clone(),
            stop: stop.clone(),
            failed: failed.clone(),
            process: process.clone(),
        };
        std::thread::Builder::new()
            .name("device-audio-playback".into())
            .spawn(move || {
                let result = (|| -> Result<()> {
                    let mut decoder = crate::playback::Decoder::new()?;
                    let mut jitter = crate::playback::Prebuffer::new(RATE);
                    let mut queue = VecDeque::<f32>::new();
                    let mut decoded = Vec::new();
                    let mut primed = false;
                    let mut due = Instant::now();
                    let mut raw = [0u8; SAMPLES * 2 * 4];
                    while !stop.load(Ordering::Acquire) {
                        ensure!(process.running(), "audio output ended");
                        loop {
                            let packet = packets.lock().unwrap().pop_front();
                            let Some(packet) = packet else { break };
                            if packet.created.elapsed() > Duration::from_millis(80) {
                                stats.lock().unwrap().packet_drops += 1;
                                continue;
                            }
                            let (missing, concealed) = decoder.decode(
                                packet.sequence,
                                &packet.data,
                                !queue.is_empty(),
                                &mut decoded,
                            )?;
                            let mut counters = stats.lock().unwrap();
                            counters.missing += missing;
                            counters.concealed += concealed;
                            queue.extend(decoded.iter().map(|v| {
                                if v.is_finite() {
                                    v.clamp(-1.0, 1.0)
                                } else {
                                    0.0
                                }
                            }));
                            while queue.len() > RATE as usize * 2 * 120 / 1000 {
                                queue.pop_front();
                                queue.pop_front();
                                counters.pcm_drops += 1;
                            }
                        }
                        if !primed && queue.len() / 2 >= jitter.frames(SAMPLES) {
                            primed = true;
                        }
                        let now = Instant::now();
                        if now < due {
                            std::thread::sleep((due - now).min(Duration::from_millis(2)));
                            continue;
                        }
                        if now.saturating_duration_since(due) > Duration::from_millis(30) {
                            due = now;
                        }
                        let play = primed && queue.len() >= SAMPLES * 2;
                        if primed && !play {
                            primed = false;
                            jitter.underrun();
                            stats.lock().unwrap().underruns += 1;
                        }
                        for bytes in raw.chunks_exact_mut(4) {
                            bytes.copy_from_slice(
                                &if play {
                                    queue.pop_front().unwrap()
                                } else {
                                    0.0f32
                                }
                                .to_le_bytes(),
                            );
                        }
                        input.write_all(&raw)?;
                        if play {
                            jitter.played(SAMPLES);
                        }
                        let mut counters = stats.lock().unwrap();
                        counters.queued_ms = queue.len() as u64 * 1000 / (RATE as u64 * 2);
                        counters.prebuffer_ms = jitter.milliseconds as u64;
                        counters.callback_max = SAMPLES as u64;
                        due += Duration::from_millis(10);
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    eprintln!("PaneFerry audio playback: {error:#}");
                    failed.store(true, Ordering::Release);
                }
                process.stop();
            })?;
        Ok(player)
    }
    pub fn push(&self, packet: Packet) -> Result<()> {
        ensure!(self.healthy(), "audio output unavailable");
        ensure!(
            !packet.data.is_empty() && packet.data.len() <= MAX_PACKET,
            "invalid audio packet"
        );
        let mut queue = self.packets.lock().unwrap();
        if queue.len() == 16 {
            queue.pop_front();
            self.stats.lock().unwrap().packet_drops += 1;
        }
        queue.push_back(packet);
        Ok(())
    }
    pub fn healthy(&self) -> bool {
        !self.failed.load(Ordering::Acquire)
    }
    pub fn stats(&self) -> PlaybackSnapshot {
        let mut counters = self.stats.lock().unwrap();
        let snapshot = std::mem::take(&mut *counters);
        *counters = PlaybackSnapshot {
            sample_rate: snapshot.sample_rate,
            queued_ms: snapshot.queued_ms,
            prebuffer_ms: snapshot.prebuffer_ms,
            ..Default::default()
        };
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a private Pulse server and a test-owned null sink; never use the login output"]
    fn private_monitor_shared_opus_playback_and_cleanup() {
        let server = std::env::var("PULSE_SERVER").unwrap();
        ensure_private(&server);
        let mut first = subscribe().unwrap();
        let second = subscribe().unwrap();
        assert!(Arc::ptr_eq(&first._capture, &second._capture));
        let capture = first._capture.process.clone();
        let player = Player::start().unwrap();
        let output = player.process.clone();
        let mut codec = crate::encoder().unwrap();
        let mut encoded = [0; MAX_PACKET];
        let mut decoder = opus::Decoder::new(RATE, opus::Channels::Stereo).unwrap();
        let mut pcm = [0.0; SAMPLES * 2];
        let mut peak = 0.0f32;
        let mut count = 0;
        for sequence in 1..=160 {
            let samples: Vec<_> = (0..SAMPLES)
                .flat_map(|i| {
                    let v = (((sequence - 1) * SAMPLES as u64 + i as u64) as f32
                        * 440.0
                        * std::f32::consts::TAU
                        / RATE as f32)
                        .sin()
                        * 0.1;
                    [v; 2]
                })
                .collect();
            let n = codec.encode_float(&samples, &mut encoded).unwrap();
            player
                .push(Packet {
                    sequence,
                    created: Instant::now(),
                    data: encoded[..n].to_vec(),
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(10));
            while let Ok(packet) = first.receiver.try_recv() {
                let packet = packet.unwrap();
                decoder.decode_float(&packet.data, &mut pcm, false).unwrap();
                peak = peak.max((pcm.iter().map(|v| v * v).sum::<f32>() / pcm.len() as f32).sqrt());
                count += 1;
            }
        }
        drop(player);
        assert!(!output.running());
        drop(first);
        assert!(capture.running());
        drop(second);
        assert!(!capture.running());
        assert!(
            peak > 0.01 && count > 60,
            "silent monitor: packets={count} rms={peak}"
        );
        eprintln!("PASS private monitor: packets={count} peak_rms={peak:.4}; shared capture, Opus playback and last-subscriber cleanup");
    }
    fn ensure_private(server: &str) {
        assert!(
            server.starts_with("unix:/tmp/paneferry-audio-"),
            "refusing to record the login server"
        );
    }
}
