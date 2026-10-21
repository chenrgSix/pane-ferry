use anyhow::{bail, Result};
use std::borrow::Cow;
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub bpp: u8,
    pub pad: u8,
    pub little: bool,
    pub masks: (u32, u32, u32),
}
impl Image {
    pub fn pixels(&self) -> usize {
        self.width as usize * self.height as usize
    }
    pub fn png(&self) -> Result<Vec<u8>> {
        encode(
            self.width,
            self.height,
            &self.data,
            self.bpp,
            self.pad,
            self.little,
            self.masks,
        )
    }
    pub fn bgrx(&self) -> Result<Cow<'_, [u8]>> {
        remoteapp_protocol::dimensions(self.width, self.height)?;
        if self.bpp == 32 && self.pad == 32 && self.little && self.masks == (0xff0000, 0xff00, 0xff)
        {
            let length = self.pixels() * 4;
            if self.data.len() < length {
                bail!("short X11 image")
            }
            return Ok(Cow::Borrowed(&self.data[..length]));
        }
        let rgb = rgb_pixels(
            self.width,
            self.height,
            &self.data,
            self.bpp,
            self.pad,
            self.little,
            self.masks,
        )?;
        let mut pixels = Vec::with_capacity(self.pixels() * 4);
        for pixel in rgb.chunks_exact(3) {
            pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0]);
        }
        Ok(Cow::Owned(pixels))
    }
}
/// Xvfb configured as 24-bit TrueColor; handles 24/32-bit scanlines and server byte order.
pub fn encode(
    w: u32,
    h: u32,
    data: &[u8],
    bpp: u8,
    pad: u8,
    little: bool,
    masks: (u32, u32, u32),
) -> Result<Vec<u8>> {
    let rgb = rgb_pixels(w, h, data, bpp, pad, little, masks)?;
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::Fast);
        e.set_filter(png::FilterType::Sub);
        let mut writer = e.write_header()?;
        writer.write_image_data(&rgb)?;
    }
    Ok(out)
}
fn rgb_pixels(
    w: u32,
    h: u32,
    data: &[u8],
    bpp: u8,
    pad: u8,
    little: bool,
    masks: (u32, u32, u32),
) -> Result<Vec<u8>> {
    remoteapp_protocol::dimensions(w, h)?;
    if ![24, 32].contains(&bpp) || ![8, 16, 32].contains(&pad) {
        bail!("unsupported X11 pixel format")
    }
    let stride = (w as usize * bpp as usize).div_ceil(pad as usize) * pad as usize / 8;
    if data.len() < stride * h as usize {
        bail!("short X11 image")
    }
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    if [masks.0, masks.1, masks.2].contains(&0) {
        bail!("invalid color mask")
    }
    let standard = little && masks == (0xff0000, 0xff00, 0xff);
    let channels =
        [masks.0, masks.1, masks.2].map(|m| (m, m.trailing_zeros(), m >> m.trailing_zeros()));
    for y in 0..h as usize {
        if standard {
            // The common Xvfb/Xwayland BGRX layout needs byte shuffling only.
            for pixel in data[y * stride..y * stride + w as usize * (bpp as usize / 8)]
                .chunks_exact(bpp as usize / 8)
            {
                rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
            continue;
        }
        for x in 0..w as usize {
            let p = &data[y * stride + x * (bpp as usize / 8)..];
            let mut n = 0u32;
            for (i, byte) in p.iter().enumerate().take(bpp as usize / 8) {
                let shift = if little {
                    i * 8
                } else {
                    (bpp as usize / 8 - 1 - i) * 8
                };
                n |= (*byte as u32) << shift;
            }
            for (m, shift, max) in channels {
                let v = (n & m) >> shift;
                rgb.push((v as u64 * 255 / max as u64) as u8);
            }
        }
    }
    Ok(rgb)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pixel() {
        let p = encode(
            1,
            1,
            &[0, 0, 255, 0],
            32,
            32,
            true,
            (0xff0000, 0xff00, 0xff),
        )
        .unwrap();
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
    }
    #[test]
    fn short() {
        assert!(encode(10, 10, &[], 32, 32, true, (1, 2, 4)).is_err());
    }
    #[test]
    fn preserves_pixels_for_padded_bgr_and_big_endian_masks() {
        for (data, bpp, little, masks) in [
            (
                vec![3, 2, 1, 0, 6, 5, 4, 0],
                24,
                true,
                (0xff0000, 0xff00, 0xff),
            ),
            (
                vec![0, 1, 2, 3, 0, 4, 5, 6],
                32,
                false,
                (0xff0000, 0xff00, 0xff),
            ),
        ] {
            let encoded = encode(1, 2, &data, bpp, 32, little, masks).unwrap();
            let mut reader = png::Decoder::new(encoded.as_slice()).read_info().unwrap();
            let mut pixels = vec![0; reader.output_buffer_size()];
            reader.next_frame(&mut pixels).unwrap();
            assert_eq!(pixels, [1, 2, 3, 4, 5, 6]);
            let image = Image {
                width: 1,
                height: 2,
                data,
                bpp,
                pad: 32,
                little,
                masks,
            };
            assert_eq!(image.bgrx().unwrap().as_ref(), [3, 2, 1, 0, 6, 5, 4, 0]);
        }
    }
}
