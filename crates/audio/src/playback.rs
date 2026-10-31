//! Preserve useful PCM across isolated packet gaps; never reset the playback
//! queue just because the transport discarded one late packet.
use crate::{RATE, SAMPLES};
use anyhow::{ensure, Result};

pub(crate) struct Decoder {
    codec: opus::Decoder,
    sequence: Option<u64>,
    pcm: [f32; SAMPLES * 2],
}
impl Decoder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            codec: opus::Decoder::new(RATE, opus::Channels::Stereo)?,
            sequence: None,
            pcm: [0.0; SAMPLES * 2],
        })
    }
    pub fn decode(
        &mut self,
        sequence: u64,
        packet: &[u8],
        conceal: bool,
        output: &mut Vec<f32>,
    ) -> Result<(u64, u64)> {
        let missing = if let Some(last) = self.sequence {
            ensure!(sequence > last, "audio sequence went backwards");
            sequence - last - 1
        } else {
            0
        };
        output.clear();
        let mut concealed = 0;
        if missing <= 3 {
            for _ in 0..missing {
                self.codec.decode_float(&[], &mut self.pcm, false)?;
                // If the device already ran dry, those samples' playback time
                // has passed. Advance codec state without replaying the gap.
                if conceal {
                    output.extend_from_slice(&self.pcm);
                    concealed += 1;
                }
            }
        } else {
            self.codec.reset_state()?;
        }
        let count = self.codec.decode_float(packet, &mut self.pcm, false)?;
        ensure!(count == SAMPLES, "invalid audio duration");
        output.extend_from_slice(&self.pcm);
        self.sequence = Some(sequence);
        Ok((missing, concealed))
    }
}

pub(crate) struct Prebuffer {
    pub milliseconds: u32,
    stable_frames: usize,
    rate: u32,
}
impl Prebuffer {
    pub fn new(rate: u32) -> Self {
        Self {
            milliseconds: 40,
            stable_frames: 0,
            rate,
        }
    }
    pub fn frames(&self, callback_frames: usize) -> usize {
        (self.rate as usize * self.milliseconds as usize / 1000).max(callback_frames)
    }
    pub fn underrun(&mut self) {
        self.milliseconds = (self.milliseconds + 10).min(80);
        self.stable_frames = 0;
    }
    pub fn played(&mut self, frames: usize) {
        self.stable_frames += frames;
        if self.stable_frames >= self.rate as usize * 10 {
            self.milliseconds = self.milliseconds.saturating_sub(10).max(40);
            self.stable_frames = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_loss_is_concealed_but_long_gaps_cannot_build_latency() {
        let mut encoder = crate::encoder().unwrap();
        let mut encoded = [0; crate::MAX_PACKET];
        let mut decoder = Decoder::new().unwrap();
        let mut output = Vec::new();
        for sequence in 1..=30 {
            let pcm: Vec<f32> = (0..SAMPLES)
                .flat_map(|i| {
                    let v = (((sequence - 1) * SAMPLES as u64 + i as u64) as f32
                        * 440.0
                        * std::f32::consts::TAU
                        / RATE as f32)
                        .sin()
                        * 0.2;
                    [v; 2]
                })
                .collect();
            let size = encoder.encode_float(&pcm, &mut encoded).unwrap();
            if ![1, 3, 5, 30].contains(&sequence) {
                continue;
            }
            let (lost, concealed) = decoder
                .decode(sequence, &encoded[..size], sequence != 5, &mut output)
                .unwrap();
            if sequence == 3 {
                assert_eq!((lost, concealed, output.len()), (1, 1, SAMPLES * 4));
            }
            if sequence == 5 {
                assert_eq!((lost, concealed, output.len()), (1, 0, SAMPLES * 2));
            }
            if sequence == 30 {
                assert_eq!((lost, concealed, output.len()), (24, 0, SAMPLES * 2));
            }
            assert!(output.iter().all(|v| v.is_finite()));
        }
    }
    #[test]
    fn thirty_ms_delivery_jitter_no_longer_drains_playback() {
        fn simulate(initial_ms: u32) -> usize {
            let mut target = Prebuffer::new(RATE);
            target.milliseconds = initial_ms;
            let mut arrivals = Vec::new();
            let mut previous = 0;
            for packet in 0..200 {
                // An ordered transport holds later packets behind a delayed one.
                previous = previous.max(packet * 10 + if packet % 40 == 20 { 30 } else { 0 });
                arrivals.push(previous);
            }
            let (mut next, mut queued, mut underruns) = (0, 0usize, 0);
            let mut primed = false;
            for time in (0..1950).step_by(10) {
                while next < arrivals.len() && arrivals[next] <= time {
                    queued += SAMPLES;
                    next += 1;
                }
                if !primed && queued >= target.frames(SAMPLES) {
                    primed = true;
                }
                if primed {
                    if queued >= SAMPLES {
                        queued -= SAMPLES;
                        target.played(SAMPLES);
                    } else {
                        underruns += 1;
                        primed = false;
                        target.underrun();
                    }
                }
            }
            underruns
        }
        assert!(simulate(20) > 0);
        assert_eq!(simulate(40), 0);
    }
    #[test]
    fn jitter_target_is_bounded_and_recovers_after_stable_playback() {
        let mut jitter = Prebuffer::new(48_000);
        assert_eq!(jitter.frames(480), 1920);
        assert_eq!(jitter.frames(4096), 4096);
        for _ in 0..20 {
            jitter.underrun();
        }
        assert_eq!(jitter.milliseconds, 80);
        for _ in 0..50 {
            jitter.played(48_000);
        }
        assert_eq!(jitter.milliseconds, 40);
    }
}
