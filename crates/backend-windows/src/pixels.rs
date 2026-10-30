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
    let (luma, chroma) = output.split_at_mut(w * h);
    let y_value = |p: &[u8]| {
        (((66 * p[2] as i32 + 129 * p[1] as i32 + 25 * p[0] as i32 + 128) >> 8) + 16) as u8
    };
    // Visit each BGRA pixel once, computing both planes in the same 2x2 block.
    // Only odd right/bottom edges need clamping; the common even path has none.
    for y in (0..h).step_by(2) {
        let row0 = &bgra[y.min(height as usize - 1) * width as usize * 4..][..width as usize * 4];
        let row1 =
            &bgra[(y + 1).min(height as usize - 1) * width as usize * 4..][..width as usize * 4];
        let (above, below) = luma[y * w..(y + 2) * w].split_at_mut(w);
        let uv = &mut chroma[y / 2 * w..][..w];
        for x in (0..w).step_by(2) {
            let x0 = x.min(width as usize - 1) * 4;
            let x1 = (x + 1).min(width as usize - 1) * 4;
            let p0 = &row0[x0..x0 + 4];
            let p1 = &row0[x1..x1 + 4];
            let p2 = &row1[x0..x0 + 4];
            let p3 = &row1[x1..x1 + 4];
            above[x] = y_value(p0);
            above[x + 1] = y_value(p1);
            below[x] = y_value(p2);
            below[x + 1] = y_value(p3);
            let r = (p0[2] as i32 + p1[2] as i32 + p2[2] as i32 + p3[2] as i32) / 4;
            let g = (p0[1] as i32 + p1[1] as i32 + p2[1] as i32 + p3[1] as i32) / 4;
            let b = (p0[0] as i32 + p1[0] as i32 + p2[0] as i32 + p3[0] as i32) / 4;
            uv[x] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
            uv[x + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
        }
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saturated_colors_and_odd_edges_retain_bt601_values() {
        assert_eq!(
            nv12(&[0, 0, 255, 255], 1, 1).unwrap(),
            [82, 82, 82, 82, 90, 240]
        );
        assert_eq!(
            nv12(&[0, 255, 0, 255], 1, 1).unwrap(),
            [144, 144, 144, 144, 54, 34]
        );
        assert_eq!(
            nv12(&[255, 0, 0, 255], 1, 1).unwrap(),
            [41, 41, 41, 41, 240, 110]
        );
        let image = [0, 0, 255, 255, 0, 0, 255, 255, 255, 0, 0, 255];
        assert_eq!(
            nv12(&image, 3, 1).unwrap(),
            [82, 82, 41, 41, 82, 82, 41, 41, 90, 240, 240, 110]
        );
    }
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
