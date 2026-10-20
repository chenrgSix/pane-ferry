use anyhow::{bail, Result};
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
    remoteapp_protocol::dimensions(w, h)?;
    if ![24, 32].contains(&bpp) || pad == 0 {
        bail!("unsupported X11 pixel format")
    }
    let stride = (w as usize * bpp as usize).div_ceil(pad as usize) * pad as usize / 8;
    if data.len() < stride * h as usize {
        bail!("short X11 image")
    }
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
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
            for m in [masks.0, masks.1, masks.2] {
                if m == 0 {
                    bail!("invalid color mask")
                }
                let v = (n & m) >> m.trailing_zeros();
                let max = m >> m.trailing_zeros();
                rgb.push((v as u64 * 255 / max as u64) as u8);
            }
        }
    }
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        let mut writer = e.write_header()?;
        writer.write_image_data(&rgb)?;
    }
    Ok(out)
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
}
