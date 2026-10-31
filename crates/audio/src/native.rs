use crate::{Packet, Resampler, MAX_PACKET, RATE, SAMPLES};
use anyhow::{bail, ensure, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_queue::ArrayQueue;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

fn output() -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let device = cpal::default_host()
        .default_output_device()
        .context("no default output device")?;
    let config = device.default_output_config()?;
    ensure!(
        (1..=8).contains(&config.channels()),
        "unsupported audio channels"
    );
    ensure!(
        (8_000..=192_000).contains(&config.sample_rate().0),
        "unsupported audio rate"
    );
    Ok((device, config))
}
fn device_changed(name: &str) -> bool {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.name().ok())
        .as_deref()
        != Some(name)
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct Sample {
    stereo: [f32; 2],
    captured: Instant,
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "plays a quiet 440 Hz tone on the real default output device"]
    fn real_loopback_opus_playback_and_last_subscriber_cleanup() {
        let mut first = subscribe().unwrap();
        let second = subscribe().unwrap();
        assert!(Arc::ptr_eq(&first._capture, &second._capture));
        let capture_stop = first._capture.stop.clone();
        let player = Player::start().unwrap();
        let playback_stop = player.stop.clone();
        let mut encoder = crate::encoder().unwrap();
        let mut decoder = opus::Decoder::new(RATE, opus::Channels::Stereo).unwrap();
        let mut encoded = [0; MAX_PACKET];
        let mut decoded = [0.0; SAMPLES * 2];
        let mut peak_rms = 0.0f32;
        let mut packets = 0;
        for sequence in 1..=100 {
            let pcm: Vec<f32> = (0..SAMPLES)
                .flat_map(|i| {
                    let phase = ((sequence - 1) as usize * SAMPLES + i) as f32
                        * 440.0
                        * std::f32::consts::TAU
                        / RATE as f32;
                    [phase.sin() * 0.03; 2]
                })
                .collect();
            let count = encoder.encode_float(&pcm, &mut encoded).unwrap();
            player
                .push(Packet {
                    sequence,
                    created: Instant::now(),
                    data: encoded[..count].to_vec(),
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(10));
            while let Ok(packet) = first.receiver.try_recv() {
                let packet = packet.unwrap();
                let count = decoder
                    .decode_float(&packet.data, &mut decoded, false)
                    .unwrap();
                assert_eq!(count, SAMPLES);
                let rms =
                    (decoded.iter().map(|v| v * v).sum::<f32>() / decoded.len() as f32).sqrt();
                peak_rms = peak_rms.max(rms);
                packets += 1;
            }
        }
        drop(player);
        assert!(playback_stop.load(Ordering::Acquire));
        drop(first);
        assert!(!capture_stop.load(Ordering::Acquire));
        drop(second);
        assert!(capture_stop.load(Ordering::Acquire));
        eprintln!("loopback packets={packets} peak_rms={peak_rms:.5}; shared capture and stop guards verified");
        assert!(packets >= 50, "not enough captured audio packets");
        assert!(peak_rms > 0.001, "system output was silent or muted");
    }
}

#[cfg(windows)]
fn input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<ArrayQueue<Sample>>,
    failed: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            let captured = Instant::now();
            for frame in data.chunks_exact(channels) {
                let get = |index: usize| {
                    frame
                        .get(index)
                        .map(|v| v.to_sample::<f32>())
                        .unwrap_or(0.0)
                };
                let stereo = if channels == 1 {
                    [get(0); 2]
                } else if channels == 2 {
                    [get(0), get(1)]
                } else if channels == 4 {
                    [
                        (get(0) + 0.707 * get(2)) / 1.707,
                        (get(1) + 0.707 * get(3)) / 1.707,
                    ]
                } else {
                    // WASAPI's usual 5.1/7.1 channel order: FL FR FC LFE BL BR SL SR.
                    // Exclude LFE; normalize the mix to avoid clipping at full scale.
                    let center = get(2) * 0.707;
                    let scale = 1.0
                        + 0.707
                        + if channels > 4 { 0.707 } else { 0.0 }
                        + if channels > 6 { 0.707 } else { 0.0 };
                    [
                        (get(0) + center + 0.707 * (get(4) + get(6))) / scale,
                        (get(1) + center + 0.707 * (get(5) + get(7))) / scale,
                    ]
                };
                queue.force_push(Sample { stereo, captured });
            }
        },
        move |_| {
            failed.store(true, Ordering::Release);
        },
        None,
    )?)
}

#[cfg(windows)]
struct Capture {
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    sender: tokio::sync::broadcast::Sender<Result<Packet, String>>,
}
#[cfg(windows)]
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
#[cfg(windows)]
pub struct Subscription {
    _capture: Arc<Capture>,
    pub receiver: tokio::sync::broadcast::Receiver<Result<Packet, String>>,
}

/// All authenticated subscribers share one WASAPI stream and one encoder.
/// Call on a blocking worker; initialization is bounded and owns no runtime task.
#[cfg(windows)]
pub fn subscribe() -> Result<Subscription> {
    use std::sync::{Mutex, OnceLock, Weak};
    static CURRENT: OnceLock<Mutex<Weak<Capture>>> = OnceLock::new();
    let mut current = CURRENT
        .get_or_init(|| Mutex::new(Weak::new()))
        .lock()
        .unwrap();
    if let Some(capture) = current
        .upgrade()
        .filter(|c| !c.failed.load(Ordering::Acquire))
    {
        return Ok(Subscription {
            receiver: capture.sender.subscribe(),
            _capture: capture,
        });
    }
    let (sender, receiver) = tokio::sync::broadcast::channel(8);
    let capture = Arc::new(Capture {
        stop: Arc::new(AtomicBool::new(false)),
        failed: Arc::new(AtomicBool::new(false)),
        sender: sender.clone(),
    });
    let stop = capture.stop.clone();
    let failed = capture.failed.clone();
    let (ready, wait) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("system-audio-capture".into())
        .spawn(move || {
            if let Err(error) = capture_loop(stop, failed.clone(), &sender, &ready) {
                failed.store(true, Ordering::Release);
                let _ = ready.try_send(Err(error.to_string()));
                let _ = sender.send(Err(error.to_string()));
            }
        })?;
    wait.recv_timeout(Duration::from_secs(5))?
        .map_err(anyhow::Error::msg)?;
    *current = Arc::downgrade(&capture);
    Ok(Subscription {
        _capture: capture,
        receiver,
    })
}

#[cfg(windows)]
fn capture_loop(
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    sender: &tokio::sync::broadcast::Sender<Result<Packet, String>>,
    ready: &mpsc::SyncSender<Result<(), String>>,
) -> Result<()> {
    let (device, supported) = output()?;
    let name = device.name()?;
    let config: cpal::StreamConfig = supported.clone().into();
    ensure!(
        matches!(config.channels, 1 | 2 | 4 | 6 | 8),
        "unsupported capture channel layout"
    );
    let queue = Arc::new(ArrayQueue::new(config.sample_rate.0 as usize / 10));
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => {
            input_stream::<f32>(&device, &config, queue.clone(), failed.clone())?
        }
        cpal::SampleFormat::I16 => {
            input_stream::<i16>(&device, &config, queue.clone(), failed.clone())?
        }
        cpal::SampleFormat::I32 => {
            input_stream::<i32>(&device, &config, queue.clone(), failed.clone())?
        }
        cpal::SampleFormat::U16 => {
            input_stream::<u16>(&device, &config, queue.clone(), failed.clone())?
        }
        cpal::SampleFormat::F64 => {
            input_stream::<f64>(&device, &config, queue.clone(), failed.clone())?
        }
        _ => bail!("unsupported audio format"),
    };
    let mut resampler = Resampler::new(config.sample_rate.0, RATE)?;
    let mut encoder = crate::encoder()?;
    let mut pcm = Vec::with_capacity(SAMPLES * 4);
    let mut encoded = [0u8; MAX_PACKET];
    let mut sequence = 0;
    let mut checked = Instant::now();
    stream.play()?;
    let _ = ready.send(Ok(()));
    while !stop.load(Ordering::Acquire) {
        ensure!(
            !failed.load(Ordering::Acquire),
            "audio output device unavailable"
        );
        if checked.elapsed() >= Duration::from_secs(1) {
            ensure!(!device_changed(&name), "audio output device changed");
            checked = Instant::now();
        }
        if let Some(sample) = queue.pop() {
            if sample.captured.elapsed() > Duration::from_millis(80) {
                pcm.clear();
                resampler = Resampler::new(config.sample_rate.0, RATE)?;
                continue;
            }
            resampler.push(sample.stereo, &mut pcm);
            while pcm.len() >= SAMPLES * 2 {
                let size = encoder.encode_float(&pcm[..SAMPLES * 2], &mut encoded)?;
                pcm.drain(..SAMPLES * 2);
                sequence += 1;
                let _ = sender.send(Ok(Packet {
                    sequence,
                    created: sample.captured,
                    data: encoded[..size].to_vec(),
                }));
            }
        } else {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}

pub struct Player {
    sender: mpsc::SyncSender<Packet>,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}
impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
impl Player {
    /// Own the non-Send CoreAudio/WASAPI stream entirely on its playback thread.
    pub fn start() -> Result<Self> {
        let (sender, receive) = mpsc::sync_channel(8);
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let player = Self {
            sender,
            stop: stop.clone(),
            failed: failed.clone(),
        };
        let (ready, wait) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("device-audio-playback".into())
            .spawn(move || {
                if let Err(error) = playback_loop(stop, failed.clone(), receive, &ready) {
                    failed.store(true, Ordering::Release);
                    let _ = ready.try_send(Err(error.to_string()));
                }
            })?;
        wait.recv_timeout(Duration::from_secs(5))?
            .map_err(anyhow::Error::msg)?;
        Ok(player)
    }
    pub fn push(&self, packet: Packet) -> Result<()> {
        ensure!(
            !self.failed.load(Ordering::Acquire),
            "audio playback device unavailable"
        );
        ensure!(
            !packet.data.is_empty() && packet.data.len() <= MAX_PACKET,
            "invalid audio packet"
        );
        // Backpressure restarts the subscription; it never builds an unbounded delay.
        self.sender
            .try_send(packet)
            .context("audio playback queue full or closed")?;
        Ok(())
    }
    pub fn healthy(&self) -> bool {
        !self.failed.load(Ordering::Acquire)
    }
}

fn output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<ArrayQueue<[f32; 2]>>,
    failed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let prebuffer = config.sample_rate.0 as usize / 50; // 20 ms after startup/underrun.
    let mut primed = false;
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            if !primed && queue.len() >= prebuffer {
                primed = true;
            }
            for frame in data.chunks_exact_mut(channels) {
                let stereo = if primed && !stop.load(Ordering::Acquire) {
                    queue.pop().unwrap_or_else(|| {
                        primed = false;
                        [0.0; 2]
                    })
                } else {
                    [0.0; 2]
                };
                for (index, sample) in frame.iter_mut().enumerate() {
                    let value = if channels == 1 {
                        (stereo[0] + stereo[1]) * 0.5
                    } else {
                        stereo.get(index).copied().unwrap_or(0.0)
                    };
                    *sample = T::from_sample(value.clamp(-1.0, 1.0));
                }
            }
        },
        move |_| {
            failed.store(true, Ordering::Release);
        },
        None,
    )?)
}
fn playback_loop(
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    receive: mpsc::Receiver<Packet>,
    ready: &mpsc::SyncSender<Result<(), String>>,
) -> Result<()> {
    let (device, supported) = output()?;
    let name = device.name()?;
    let config: cpal::StreamConfig = supported.clone().into();
    let queue = Arc::new(ArrayQueue::new(config.sample_rate.0 as usize / 10));
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => output_stream::<f32>(
            &device,
            &config,
            queue.clone(),
            failed.clone(),
            stop.clone(),
        )?,
        cpal::SampleFormat::I16 => output_stream::<i16>(
            &device,
            &config,
            queue.clone(),
            failed.clone(),
            stop.clone(),
        )?,
        cpal::SampleFormat::I32 => output_stream::<i32>(
            &device,
            &config,
            queue.clone(),
            failed.clone(),
            stop.clone(),
        )?,
        cpal::SampleFormat::U16 => output_stream::<u16>(
            &device,
            &config,
            queue.clone(),
            failed.clone(),
            stop.clone(),
        )?,
        cpal::SampleFormat::F64 => output_stream::<f64>(
            &device,
            &config,
            queue.clone(),
            failed.clone(),
            stop.clone(),
        )?,
        _ => bail!("unsupported audio playback format"),
    };
    let mut decoder = opus::Decoder::new(RATE, opus::Channels::Stereo)?;
    let mut resampler = Resampler::new(RATE, config.sample_rate.0)?;
    let mut pcm = [0.0; SAMPLES * 2];
    let mut converted = Vec::with_capacity(SAMPLES * 8);
    let mut sequence = None;
    let mut checked = Instant::now();
    stream.play()?;
    let _ = ready.send(Ok(()));
    while !stop.load(Ordering::Acquire) {
        ensure!(!failed.load(Ordering::Acquire), "audio playback failed");
        if checked.elapsed() >= Duration::from_secs(1) {
            ensure!(!device_changed(&name), "audio playback device changed");
            checked = Instant::now();
        }
        match receive.recv_timeout(Duration::from_millis(20)) {
            Ok(packet) => {
                if packet.created.elapsed() > Duration::from_millis(80) {
                    continue;
                }
                if sequence.is_some_and(|last| packet.sequence != last + 1) {
                    decoder.reset_state()?;
                    resampler = Resampler::new(RATE, config.sample_rate.0)?;
                    while queue.pop().is_some() {}
                }
                sequence = Some(packet.sequence);
                let count = decoder.decode_float(&packet.data, &mut pcm, false)?;
                ensure!(count == SAMPLES, "invalid audio duration");
                converted.clear();
                for frame in pcm.chunks_exact(2) {
                    resampler.push([frame[0], frame[1]], &mut converted);
                }
                for frame in converted.chunks_exact(2) {
                    queue.force_push([frame[0], frame[1]]);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}
