//! Copy Core Image's RGBA float bands into the 3-float result. Pure and safe: the destination is
//! only written through slices, never by Core Image.

/// Rows per band: 256 × 7008 × 16 B = 28.7 MB of scratch at full size.
pub(crate) const BAND_ROWS: u32 = 256;

/// One RGBAf pixel of Core Image's scratch, 16-byte aligned (`render:toBitmap:` may use vector stores).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C, align(16))]
pub(crate) struct Rgba(pub [f32; 4]);

/// `n` zeroed values, or `None` when the memory can't be had (never an abort on a huge picture).
pub(crate) fn zeroed<T: Copy + Default>(n: usize) -> Option<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).ok()?;
    v.resize(n, T::default());
    Some(v)
}

/// Drop alpha; non-finite and negative values become 0 (measured minimum −0.005).
pub(crate) fn rgba_to_rgb(src: &[Rgba], dst: &mut [[f32; 3]]) {
    for (s, d) in src.iter().zip(dst.iter_mut()) {
        for (o, v) in d.iter_mut().zip(&s.0) {
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
    use super::{Rgba, bands, rgba_to_rgb, zeroed};

    #[test]
    fn alpha_dropped_and_values_cleaned() {
        let src = [Rgba([0.5, 1.0, 3.25, 1.0]), Rgba([-0.005, f32::NAN, f32::INFINITY, 0.0])];
        let mut dst = [[9.0; 3]; 2];
        rgba_to_rgb(&src, &mut dst);
        assert_eq!(dst, [[0.5, 1.0, 3.25], [0.0, 0.0, 0.0]]);
    }

    /// Review fix 9: the scratch Core Image draws into is 16-byte aligned, and a buffer too large to have is an error.
    #[test]
    fn scratch_is_aligned_and_allocation_can_fail() {
        let scratch: Vec<Rgba> = zeroed(33).unwrap();
        assert_eq!(scratch.as_ptr() as usize % 16, 0);
        assert_eq!((std::mem::size_of::<Rgba>(), scratch.len()), (16, 33));
        assert!(zeroed::<[f32; 3]>(usize::MAX / 8).is_none());
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
