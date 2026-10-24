use anyhow::{ensure, Result};

/// BGRA to limited-range BT.601 NV12. Odd edges are replicated, not stretched.
pub fn nv12(bgra: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    paneferry_protocol::dimensions(width, height)?;
    ensure!(
        bgra.len() == width as usize * height as usize * 4,
        "invalid pixels"
    );
    let w = (width + width % 2) as usize;
    let h = (height + height % 2) as usize;
    let mut output = vec![0; w * h * 3 / 2];
    let pixel = |x: usize, y: usize| {
        let i = (y.min(height as usize - 1) * width as usize + x.min(width as usize - 1)) * 4;
        (bgra[i + 2] as i32, bgra[i + 1] as i32, bgra[i] as i32)
    };
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = pixel(x, y);
            output[y * w + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).clamp(0, 255) as u8;
        }
    }
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            let (mut r, mut g, mut b) = (0, 0, 0);
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = pixel(x + dx, y + dy);
                    r += p.0;
                    g += p.1;
                    b += p.2;
                }
            }
            r /= 4;
            g /= 4;
            b /= 4;
            let i = w * h + (y / 2) * w + x;
            output[i] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
            output[i + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128).clamp(0, 255) as u8;
        }
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn black_white_and_odd_edges_have_valid_nv12_planes() {
        assert_eq!(
            nv12(&[0, 0, 0, 255], 1, 1).unwrap(),
            [16, 16, 16, 16, 128, 128]
        );
        assert_eq!(
            nv12(&[255, 255, 255, 255], 1, 1).unwrap(),
            [235, 235, 235, 235, 128, 128]
        );
        assert!(nv12(&[], 4097, 1).is_err());
        assert!(nv12(&[0; 3], 1, 1).is_err());
    }
}
