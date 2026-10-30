use anyhow::{ensure, Result};

/// BGRA to limited-range BT.601 NV12. Odd edges are replicated, not stretched.
#[cfg(test)]
pub fn nv12(bgra: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    nv12_into(bgra, width, height, &mut output)?;
    Ok(output)
}
pub fn nv12_into(bgra: &[u8], width: u32, height: u32, output: &mut Vec<u8>) -> Result<()> {
    let _timing = crate::metrics::span("video.convert");
    paneferry_protocol::dimensions(width, height)?;
    ensure!(
        bgra.len() == width as usize * height as usize * 4,
        "invalid pixels"
    );
    let w = (width + width % 2) as usize;
    let h = (height + height % 2) as usize;
    output.resize(w * h * 3 / 2, 0);
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
        #[cfg(target_arch = "x86_64")]
        let start = if std::is_x86_feature_detected!("ssse3") {
            unsafe { rows_ssse3(row0, row1, above, below, uv) }
        } else {
            0
        };
        #[cfg(not(target_arch = "x86_64"))]
        let start = 0;
        for x in (start..w).step_by(2) {
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
    Ok(())
}

/// Four pixels from each row at a time. Matches the scalar integer coefficients
/// and floor-before-chroma rounding exactly; no GPU or external codec required.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn rows_ssse3(a: &[u8], b: &[u8], ya: &mut [u8], yb: &mut [u8], uv: &mut [u8]) -> usize {
    use std::arch::x86_64::*;
    let zero = _mm_setzero_si128();
    let ycoef = _mm_setr_epi16(25, 129, 66, 0, 25, 129, 66, 0);
    let ucoef = _mm_setr_epi16(112, -74, -38, 0, 112, -74, -38, 0);
    let vcoef = _mm_setr_epi16(-18, -94, 112, 0, -18, -94, 112, 0);
    let round = _mm_set1_epi32(128);
    let end = (a.len() / 4) / 4 * 4;
    for x in (0..end).step_by(4) {
        let pa = _mm_loadu_si128(a.as_ptr().add(x * 4).cast());
        let pb = _mm_loadu_si128(b.as_ptr().add(x * 4).cast());
        let al = _mm_unpacklo_epi8(pa, zero);
        let ah = _mm_unpackhi_epi8(pa, zero);
        let bl = _mm_unpacklo_epi8(pb, zero);
        let bh = _mm_unpackhi_epi8(pb, zero);
        for (lo, hi, out) in [(al, ah, ya.as_mut_ptr()), (bl, bh, yb.as_mut_ptr())] {
            let sum = _mm_hadd_epi32(_mm_madd_epi16(lo, ycoef), _mm_madd_epi16(hi, ycoef));
            let y = _mm_add_epi32(
                _mm_srli_epi32::<8>(_mm_add_epi32(sum, round)),
                _mm_set1_epi32(16),
            );
            let packed = _mm_packus_epi16(_mm_packs_epi32(y, zero), zero);
            std::ptr::write_unaligned(out.add(x).cast::<i32>(), _mm_cvtsi128_si32(packed));
        }
        let lo = _mm_add_epi16(al, bl);
        let hi = _mm_add_epi16(ah, bh);
        let lo = _mm_add_epi16(lo, _mm_srli_si128::<8>(lo));
        let hi = _mm_add_epi16(hi, _mm_srli_si128::<8>(hi));
        let avg = _mm_srli_epi16::<2>(_mm_unpacklo_epi64(lo, hi));
        let u = _mm_hadd_epi32(_mm_madd_epi16(avg, ucoef), zero);
        let v = _mm_hadd_epi32(_mm_madd_epi16(avg, vcoef), zero);
        let chroma = _mm_add_epi32(
            _mm_srai_epi32::<8>(_mm_add_epi32(_mm_unpacklo_epi32(u, v), round)),
            round,
        );
        let packed = _mm_packus_epi16(_mm_packs_epi32(chroma, zero), zero);
        std::ptr::write_unaligned(
            uv.as_mut_ptr().add(x).cast::<i32>(),
            _mm_cvtsi128_si32(packed),
        );
    }
    end
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simd_matches_reference_for_random_odd_and_even_images() {
        for width in 1..35u32 {
            for height in 1..9u32 {
                let mut state = width * 1337 + height;
                let image: Vec<u8> = (0..width * height * 4)
                    .map(|_| {
                        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                        (state >> 24) as u8
                    })
                    .collect();
                let result = nv12(&image, width, height).unwrap();
                let w = (width + width % 2) as usize;
                let h = (height + height % 2) as usize;
                let mut expected = vec![0; w * h * 3 / 2];
                for y in 0..h {
                    for x in 0..w {
                        let offset = (y.min(height as usize - 1) * width as usize
                            + x.min(width as usize - 1))
                            * 4;
                        let p = &image[offset..];
                        expected[y * w + x] =
                            (((66 * p[2] as i32 + 129 * p[1] as i32 + 25 * p[0] as i32 + 128) >> 8)
                                + 16) as u8;
                    }
                }
                for y in (0..h).step_by(2) {
                    for x in (0..w).step_by(2) {
                        let mut rgb = [0i32; 3];
                        for dy in 0..2 {
                            for dx in 0..2 {
                                let offset = ((y + dy).min(height as usize - 1) * width as usize
                                    + (x + dx).min(width as usize - 1))
                                    * 4;
                                for c in 0..3 {
                                    rgb[c] += image[offset + c] as i32;
                                }
                            }
                        }
                        let [b, g, r] = rgb.map(|v| v / 4);
                        expected[w * h + y / 2 * w + x] =
                            (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                        expected[w * h + y / 2 * w + x + 1] =
                            (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
                    }
                }
                assert_eq!(result, expected, "{width}x{height}");
            }
        }
    }
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
