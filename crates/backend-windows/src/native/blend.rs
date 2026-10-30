//! Premultiplied BGRA over an opaque scene. No GPU or extra image allocation.

fn scalar(source: &[u8], output: &mut [u8], background: bool) {
    for (src, dst) in source.chunks_exact(4).zip(output.chunks_exact_mut(4)) {
        if background {
            dst.copy_from_slice(&[24, 24, 24, 255]);
        }
        match src[3] {
            255 => dst.copy_from_slice(src),
            0 => {}
            alpha => {
                for c in 0..3 {
                    dst[c] = (src[c] as u16 + (dst[c] as u16 * (255 - alpha as u16) + 127) / 255)
                        .min(255) as u8;
                }
                dst[3] = 255;
            }
        }
    }
}

pub(super) fn over(source: &[u8], output: &mut [u8], background: bool) {
    assert_eq!(source.len(), output.len());
    assert_eq!(source.len() % 4, 0);
    // SSE2 is part of the x86_64 baseline. Other architectures retain scalar math.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        vector(source, output, background);
    }
    #[cfg(not(target_arch = "x86_64"))]
    scalar(source, output, background);
}

#[cfg(target_arch = "x86_64")]
unsafe fn vector(source: &[u8], output: &mut [u8], background: bool) {
    use std::arch::x86_64::*;
    let zero = _mm_setzero_si128();
    let alpha_mask = _mm_set1_epi32(0xff000000u32 as i32);
    let opaque = _mm_set1_epi16(255);
    let rounding = _mm_set1_epi16(128);
    let backdrop = _mm_set1_epi32(0xff181818u32 as i32);
    let end = source.len() / 16 * 16;
    for offset in (0..end).step_by(16) {
        let src = _mm_loadu_si128(source.as_ptr().add(offset).cast());
        let alpha = _mm_and_si128(src, alpha_mask);
        if _mm_movemask_epi8(_mm_cmpeq_epi32(alpha, alpha_mask)) == 0xffff {
            _mm_storeu_si128(output.as_mut_ptr().add(offset).cast(), src);
            continue;
        }
        let dst = if background {
            backdrop
        } else {
            _mm_loadu_si128(output.as_ptr().add(offset).cast())
        };
        let blend = |s: __m128i, d: __m128i| {
            let a = _mm_shufflehi_epi16::<255>(_mm_shufflelo_epi16::<255>(s));
            let product = _mm_mullo_epi16(d, _mm_sub_epi16(opaque, a));
            // Exact rounded division by 255 for 0..65025, without division.
            let t = _mm_add_epi16(product, rounding);
            let weighted = _mm_srli_epi16::<8>(_mm_add_epi16(t, _mm_srli_epi16::<8>(t)));
            _mm_add_epi16(s, weighted)
        };
        let lo = blend(_mm_unpacklo_epi8(src, zero), _mm_unpacklo_epi8(dst, zero));
        let hi = blend(_mm_unpackhi_epi8(src, zero), _mm_unpackhi_epi8(dst, zero));
        let result = _mm_or_si128(_mm_packus_epi16(lo, hi), alpha_mask);
        // Match the scalar transparent-pixel rule even for non-premultiplied input.
        let transparent = _mm_cmpeq_epi32(alpha, zero);
        let result = _mm_or_si128(
            _mm_and_si128(transparent, dst),
            _mm_andnot_si128(transparent, result),
        );
        _mm_storeu_si128(output.as_mut_ptr().add(offset).cast(), result);
    }
    scalar(&source[end..], &mut output[end..], background);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_scalar_for_all_alpha_values_tails_and_unaligned_slices() {
        let mut seed = 42u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 24) as u8
        };
        for pixels in [1, 2, 3, 4, 5, 7, 31, 256, 257, 4097] {
            let mut source = vec![0; pixels * 4 + 1];
            let mut destination = vec![0; pixels * 4 + 3];
            for value in source.iter_mut().chain(destination.iter_mut()) {
                *value = next();
            }
            for (i, p) in source[1..].chunks_exact_mut(4).enumerate() {
                p[3] = i as u8;
            }
            for background in [false, true] {
                let mut expected = destination.clone();
                let mut actual = destination.clone();
                scalar(&source[1..], &mut expected[3..], background);
                over(&source[1..], &mut actual[3..], background);
                assert_eq!(actual, expected, "pixels={pixels} background={background}");
            }
        }
        // Exercise the entirely opaque block shortcut, including a scalar tail.
        let source = [53, 72, 210, 255].repeat(33);
        let mut output = vec![0; source.len()];
        over(&source, &mut output, true);
        assert_eq!(output, source);
    }
}
