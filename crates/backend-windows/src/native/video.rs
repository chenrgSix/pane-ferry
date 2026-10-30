use anyhow::{ensure, Result};
use std::{mem::ManuallyDrop, ptr};
use windows::{
    core::Interface,
    Win32::{
        Media::MediaFoundation::*,
        System::{Com::*, Variant::VARIANT},
    },
};

fn desktop_bitrate(width: u32, height: u32, fps: u32) -> u32 {
    // Budget scales with motion rate as well as resolution. The previous fixed
    // per-resolution budget halved bits/frame when moving from 30 to 60 fps.
    (width as u64 * height as u64 * fps.clamp(1, 90) as u64 / 6).clamp(4_000_000, 80_000_000) as u32
}

/// Built-in synchronous software encoder: no external binaries or codec download.
/// Hardware MFT selection is deliberately separate from this first implementation.
pub struct Encoder {
    transform: IMFTransform,
    header: Vec<u8>,
    time: i64,
    duration: i64,
    pub width: u32,
    pub height: u32,
    pub stream: u64,
    first: bool,
}
impl Encoder {
    pub fn new(width: u32, height: u32, fps: u32, stream: u64) -> Result<Self> {
        paneferry_protocol::dimensions(width, height)?;
        unsafe {
            let transform: IMFTransform =
                CoCreateInstance(&CMSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)?;
            let api: ICodecAPI = transform.cast()?;
            api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT::from(true))?;
            api.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &VARIANT::from(0u32))?;
            let media = MFCreateMediaType()?;
            media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            media.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
            media.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)?;
            media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
            media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            // High profile improves desktop detail at a bounded bitrate; retain
            // zero B-frames and low latency so quality does not add reorder delay.
            media.SetUINT32(&MF_MT_MPEG2_PROFILE, 100)?;
            media.SetUINT32(&MF_MT_AVG_BITRATE, desktop_bitrate(width, height, fps))?;
            transform.SetOutputType(0, &media, 0)?;
            let input = MFCreateMediaType()?;
            media.CopyAllItems(&input)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            transform.SetInputType(0, &input, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            let output = transform.GetOutputCurrentType(0)?;
            let length = output.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)?;
            ensure!(length < 65536, "invalid H264 header");
            let mut header = vec![0; length as usize];
            output.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut header, None)?;
            Ok(Self {
                transform,
                header,
                time: 0,
                duration: 10_000_000 / fps as i64,
                width,
                height,
                stream,
                first: true,
            })
        }
    }
    pub fn encode(&mut self, pixels: &[u8]) -> Result<Vec<(Vec<u8>, bool)>> {
        ensure!(
            pixels.len() == self.width as usize * self.height as usize * 3 / 2,
            "invalid NV12 frame"
        );
        unsafe {
            let input = MFCreateSample()?;
            let buffer = MFCreateMemoryBuffer(pixels.len() as u32)?;
            let mut pointer = ptr::null_mut();
            buffer.Lock(&mut pointer, None, None)?;
            ptr::copy_nonoverlapping(pixels.as_ptr(), pointer, pixels.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(pixels.len() as u32)?;
            input.AddBuffer(&buffer)?;
            input.SetSampleTime(self.time)?;
            input.SetSampleDuration(self.duration)?;
            self.time += self.duration;
            self.transform.ProcessInput(0, &input, 0)?;
            let mut frames = Vec::new();
            loop {
                let info = self.transform.GetOutputStreamInfo(0)?;
                ensure!(info.cbSize <= 12 * 1024 * 1024, "encoder output too large");
                let sample = MFCreateSample()?;
                sample.AddBuffer(&MFCreateMemoryBuffer(
                    info.cbSize.max(self.width * self.height * 3 / 2),
                )?)?;
                let mut output = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(Some(sample)),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0;
                let result = self.transform.ProcessOutput(0, &mut output, &mut status);
                let sample = ManuallyDrop::take(&mut output[0].pSample);
                drop(ManuallyDrop::take(&mut output[0].pEvents));
                if let Err(e) = result {
                    if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT {
                        break;
                    }
                    return Err(e.into());
                }
                let sample = sample.ok_or_else(|| anyhow::anyhow!("missing encoded sample"))?;
                let buffer = sample.ConvertToContiguousBuffer()?;
                let mut pointer = ptr::null_mut();
                let mut length = 0;
                buffer.Lock(&mut pointer, None, Some(&mut length))?;
                let bytes = if length <= 12 * 1024 * 1024 {
                    std::slice::from_raw_parts(pointer, length as usize).to_vec()
                } else {
                    Vec::new()
                };
                buffer.Unlock()?;
                ensure!(!bytes.is_empty(), "invalid H264 sample");
                ensure!(
                    bytes.starts_with(&[0, 0, 0, 1]) || bytes.starts_with(&[0, 0, 1]),
                    "encoder must produce Annex B"
                );
                let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
                ensure!(!self.first || key, "first frame must be an IDR");
                let mut data = Vec::new();
                if key {
                    data.extend_from_slice(&self.header);
                }
                data.extend_from_slice(&bytes);
                ensure!(data.len() <= 12 * 1024 * 1024, "H264 packet too large");
                frames.push((data, key));
                self.first = false;
                ensure!(frames.len() <= 4, "encoder backlog");
            }
            Ok(frames)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_quality_budget_tracks_frame_rate_and_stays_bounded() {
        assert_eq!(desktop_bitrate(2560, 1440, 60), 36_864_000);
        assert_eq!(desktop_bitrate(2560, 1440, 30), 18_432_000);
        assert_eq!(desktop_bitrate(64, 48, 60), 4_000_000);
        assert_eq!(desktop_bitrate(4096, 2160, 90), 80_000_000);
    }
    #[test]
    #[ignore = "runs the installed Windows H.264 encoder; opt-in performance probe"]
    fn encoder_throughput_probe() -> Result<()> {
        let _runtime = crate::CaptureRuntime::new()?;
        let (width, height) = (2560, 1440);
        let mut pixels = vec![255u8; width as usize * height as usize * 4];
        // A detailed, deterministic desktop-like grid with scrolling rows.
        for (i, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            let x = i % width as usize;
            let y = i / width as usize;
            pixel[0] = if x % 13 < 7 && y % 23 < 13 { 32 } else { 220 };
            pixel[1] = pixel[0];
            pixel[2] = pixel[0];
        }
        let mut encoder = Encoder::new(width, height, 60, 1)?;
        unsafe {
            let api: ICodecAPI = encoder.transform.cast()?;
            for (name, key) in [
                ("rate_mode", CODECAPI_AVEncCommonRateControlMode),
                ("mean_bitrate", CODECAPI_AVEncCommonMeanBitRate),
                ("quality", CODECAPI_AVEncCommonQuality),
            ] {
                let value = api.GetValue(&key)?;
                println!("ENCODER {name}={:?}", u32::try_from(&value));
            }
        }
        // Optional synthetic-only artifacts for independent decoder validation.
        let export = std::env::var_os("PANEFERRY_VIDEO_PROBE_DIR").map(std::path::PathBuf::from);
        let mut bitstream = Vec::new();
        if let Some(path) = &export {
            std::fs::create_dir_all(path)?;
        }
        let mut convert = Vec::new();
        let mut encode = Vec::new();
        let mut bytes = 0;
        let mut frames = 0;
        for _ in 0..120 {
            pixels.rotate_left(width as usize * 4);
            let start = std::time::Instant::now();
            let nv12 = crate::pixels::nv12(&pixels, width, height)?;
            let converted = std::time::Instant::now();
            if frames == 0 {
                if let Some(path) = &export {
                    std::fs::write(path.join("reference.nv12"), &nv12)?;
                }
            }
            let packets = encoder.encode(&nv12)?;
            let ended = std::time::Instant::now();
            convert.push((converted - start).as_secs_f64() * 1000.0);
            encode.push((ended - converted).as_secs_f64() * 1000.0);
            for (data, _) in packets {
                if export.is_some() {
                    bitstream.extend_from_slice(&data);
                }
                bytes += data.len();
                frames += 1;
            }
        }
        if let Some(path) = &export {
            std::fs::write(path.join("encoded.h264"), bitstream)?;
        }
        convert.sort_by(f64::total_cmp);
        encode.sort_by(f64::total_cmp);
        println!("BENCH 2560x1440@60 frames={frames} bytes={bytes} convert_p50_ms={:.2} convert_p95_ms={:.2} encode_p50_ms={:.2} encode_p95_ms={:.2}",convert[60],convert[114],encode[60],encode[114]);
        ensure!(frames > 0, "no encoded output");
        Ok(())
    }
}
