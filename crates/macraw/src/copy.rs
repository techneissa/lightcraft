//! Copy Core Image's RGBA float bands into the 3-float result. Pure and safe: the destination is
//! only written through slices, never by Core Image.

/// Rows per band: 256 × 7008 × 16 B = 28.7 MB of scratch at full size.
pub(crate) const BAND_ROWS: u32 = 256;

/// Drop alpha; non-finite and negative values become 0 (measured minimum −0.005).
pub(crate) fn rgba_to_rgb(src: &[f32], dst: &mut [f32]) {
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(3)) {
        for (o, v) in d.iter_mut().zip(s) {
            *o = if v.is_finite() && *v > 0.0 { *v } else { 0.0 };
        }
    }
}

/// The bands of a picture `height` rows tall, top to bottom: (first row, rows).
pub(crate) fn bands(height: u32, rows: u32) -> impl Iterator<Item = (u32, u32)> {
    let rows = rows.max(1);
    (0..height.div_ceil(rows)).map(move |i| {
        let top = i * rows;
        (top, rows.min(height - top))
    })
}

#[cfg(test)]
mod tests {
    use super::{bands, rgba_to_rgb};

    #[test]
    fn alpha_dropped_and_values_cleaned() {
        let src = [0.5, 1.0, 3.25, 1.0, -0.005, f32::NAN, f32::INFINITY, 0.0];
        let mut dst = [9.0; 6];
        rgba_to_rgb(&src, &mut dst);
        assert_eq!(dst, [0.5, 1.0, 3.25, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn bands_cover_every_row_once() {
        assert_eq!(bands(600, 256).collect::<Vec<_>>(), [(0, 256), (256, 256), (512, 88)]);
        assert_eq!(bands(256, 256).collect::<Vec<_>>(), [(0, 256)]);
        assert_eq!(bands(1, 256).collect::<Vec<_>>(), [(0, 1)]);
        assert_eq!(bands(0, 256).count(), 0);
        assert_eq!(bands(3, 0).count(), 3);
    }
}
