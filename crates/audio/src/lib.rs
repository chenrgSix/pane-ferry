//! Device audio: 48 kHz stereo Opus, 10 ms packets. No microphone capture.
use anyhow::{ensure, Result};
use std::{collections::VecDeque, time::Instant};

pub const RATE: u32 = 48_000;
pub const SAMPLES: usize = 480;
pub const MAX_PACKET: usize = 1275;
pub const CAPABILITY: &str = "audio.system.opus.v1";

#[derive(Clone, Debug)]
pub struct Packet {
    pub sequence: u64,
    pub created: Instant,
    pub data: Vec<u8>,
}

pub fn encoder() -> Result<opus::Encoder> {
    let mut encoder = opus::Encoder::new(RATE, opus::Channels::Stereo, opus::Application::Audio)?;
    encoder.set_bitrate(opus::Bitrate::Bits(128_000))?;
    encoder.set_complexity(5)?;
    Ok(encoder)
}

/// Stateful linear conversion; fractional phase survives callback/packet boundaries.
/// Channel mixing happens before this converter. The output is always stereo.
pub struct Resampler {
    frames: VecDeque<[f32; 2]>,
    phase: f64,
    step: f64,
}
impl Resampler {
    pub fn new(input: u32, output: u32) -> Result<Self> {
        ensure!(
            (8_000..=192_000).contains(&input) && (8_000..=192_000).contains(&output),
            "unsupported audio rate"
        );
        Ok(Self {
            frames: VecDeque::new(),
            phase: 0.0,
            step: input as f64 / output as f64,
        })
    }
    pub fn push(&mut self, frame: [f32; 2], out: &mut Vec<f32>) {
        self.frames.push_back(frame);
        while self.phase + 1.0 < self.frames.len() as f64 {
            let i = self.phase as usize;
            let fraction = (self.phase - i as f64) as f32;
            for channel in 0..2 {
                out.push(
                    self.frames[i][channel] * (1.0 - fraction)
                        + self.frames[i + 1][channel] * fraction,
                );
            }
            self.phase += self.step;
        }
        let consumed = (self.phase as usize).min(self.frames.len());
        self.frames.drain(..consumed);
        self.phase -= consumed as f64;
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod native;
#[cfg(any(windows, target_os = "macos"))]
pub use native::Player;
#[cfg(windows)]
pub use native::{subscribe, Subscription};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resampling_keeps_phase_and_channels_across_packets() {
        for input in [44_100, 48_000, 96_000] {
            let mut convert = Resampler::new(input, RATE).unwrap();
            let mut result = Vec::new();
            for _ in 0..input {
                convert.push([0.25, -0.5], &mut result);
            }
            assert!((result.len() as i64 - 96_000).abs() <= 4);
            assert!(result.chunks_exact(2).all(|s| s == [0.25, -0.5]));
        }
    }
    #[test]
    fn opus_ten_ms_stereo_roundtrip() {
        let mut encoder = encoder().unwrap();
        let mut decoder = opus::Decoder::new(RATE, opus::Channels::Stereo).unwrap();
        let pcm: Vec<_> = (0..SAMPLES)
            .flat_map(|i| {
                let signal = (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.3;
                [signal, -signal]
            })
            .collect();
        let mut packet = [0u8; MAX_PACKET];
        let mut output = [0f32; SAMPLES * 2];
        for _ in 0..10 {
            let size = encoder.encode_float(&pcm, &mut packet).unwrap();
            assert_eq!(
                decoder
                    .decode_float(&packet[..size], &mut output, false)
                    .unwrap(),
                SAMPLES
            );
        }
        assert!(output.iter().all(|v| v.is_finite() && v.abs() < 1.0));
        assert!(output.iter().any(|v| v.abs() > 0.1));
    }
}
