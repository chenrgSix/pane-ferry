use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

/// Full chroma is carried as three full-resolution planes in a negotiated H.264
/// NV12 atlas. This is not an H.264 High 4:4:4 bitstream and is still lossy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMode {
    #[default]
    #[serde(rename = "420")]
    Compatible,
    #[serde(rename = "444-atlas-v1")]
    Full,
}
impl ColorMode {
    pub fn dimensions(self, width: u32, height: u32) -> Result<(u32, u32)> {
        paneferry_protocol::dimensions(width, height)?;
        let (w, h) = (width + width % 2, height + height % 2);
        if self == Self::Compatible {
            return Ok((w, h));
        }
        // Two full planes occupy the carrier's Y plane; the third occupies UV.
        // Keep width <= 4096 rather than doubling a wide desktop to 5K/8K.
        let ew = (w * 2).min(4096);
        let rows = (2 * w * h).div_ceil(ew);
        let eh = rows + rows % 2;
        ensure!(
            eh <= 4096 && ew as u64 * eh as u64 <= 16_000_000,
            "color_mode_unavailable"
        );
        Ok((ew, eh))
    }
}

pub fn atlas_into(bgra: &[u8], width: u32, height: u32, output: &mut Vec<u8>) -> Result<()> {
    let _timing = crate::metrics::span("video.convert_444");
    let (ew, eh) = ColorMode::Full.dimensions(width, height)?;
    ensure!(
        bgra.len() == width as usize * height as usize * 4,
        "invalid BGRA frame"
    );
    let (w, h) = (width + width % 2, height + height % 2);
    let count = (w * h) as usize;
    let area = (ew * eh) as usize;
    output.resize(area * 3 / 2, 128);
    output.fill(128);
    for y in 0..h {
        for x in 0..w {
            let offset = (y.min(height - 1) * width + x.min(width - 1)) as usize * 4;
            let (b, g, r) = (
                bgra[offset] as i32,
                bgra[offset + 1] as i32,
                bgra[offset + 2] as i32,
            );
            let i = (y * w + x) as usize;
            output[i] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
            output[count + i] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            output[area + i] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atlas_keeps_every_chroma_sample_and_replicates_odd_edges() {
        for (w, h) in [(1, 1), (3, 5), (16, 8), (2560, 1440)] {
            let mut pixels = Vec::new();
            for i in 0..w * h {
                pixels.extend_from_slice(if i % 2 == 0 {
                    &[0, 0, 255, 255]
                } else {
                    &[255, 0, 0, 255]
                });
            }
            let mut data = Vec::new();
            atlas_into(&pixels, w, h, &mut data).unwrap();
            let (ew, eh) = ColorMode::Full.dimensions(w, h).unwrap();
            let pw = w + w % 2;
            let ph = h + h % 2;
            let n = (pw * ph) as usize;
            for y in 0..ph {
                for x in 0..pw {
                    let i = (y * pw + x) as usize;
                    let red = (y.min(h - 1) * w + x.min(w - 1)) % 2 == 0;
                    assert_eq!(
                        (data[i], data[n + i], data[(ew * eh) as usize + i]),
                        if red { (82, 90, 240) } else { (41, 240, 110) }
                    );
                }
            }
        }
    }
}
