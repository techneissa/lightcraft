//! Output sizes: Apple's extent to whole pixels, the requested long edge, and the caps. Pure, so
//! every system's tests cover it.

use crate::Error;

/// Largest edge accepted from Core Image (the experiment's reader used the same cap).
pub(crate) const MAX_EDGE: u32 = 20_000;
/// Largest picture, in pixels (`FIT_PREVIEW_MAX_PIXELS` in the engine).
pub(crate) const MAX_PIXELS: u64 = 128_000_000;

/// Apple's extent in whole pixels: `None` when it is empty, not finite or too large, which is how
/// Core Image answers a raw it cannot read (a wrong type hint gives 0×0).
pub(crate) fn extent_size(width: f64, height: f64) -> Option<(u32, u32)> {
    let edge = |v: f64| {
        let r = v.round();
        (r.is_finite() && r >= 1.0 && r <= f64::from(MAX_EDGE)).then_some(r as u32)
    };
    Some((edge(width)?, edge(height)?))
}

/// The size to render: `base` scaled down (never up) so its long edge is `long_edge`, aspect kept.
pub(crate) fn scaled(base: (u32, u32), long_edge: Option<u32>) -> Result<(u32, u32), Error> {
    let (w, h) = base;
    let long = w.max(h);
    let size = match long_edge {
        Some(edge) if edge < long => {
            let s = f64::from(edge.max(1)) / f64::from(long);
            let px = |v: u32| ((f64::from(v) * s).round() as u32).max(1);
            (px(w), px(h))
        }
        _ => base,
    };
    if u64::from(size.0) * u64::from(size.1) > MAX_PIXELS {
        return Err(Error::TooLarge { width: size.0, height: size.1 });
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::{MAX_EDGE, extent_size, scaled};
    use crate::Error;

    #[test]
    fn empty_or_broken_extents_are_rejected() {
        assert_eq!(extent_size(0.0, 0.0), None);
        assert_eq!(extent_size(7008.0, 0.2), None);
        assert_eq!(extent_size(f64::NAN, 10.0), None);
        assert_eq!(extent_size(f64::INFINITY, 10.0), None);
        assert_eq!(extent_size(-5.0, 10.0), None);
        assert_eq!(extent_size(f64::from(MAX_EDGE) + 1.0, 10.0), None);
        assert_eq!(extent_size(7008.0, 4671.6), Some((7008, 4672)));
    }

    #[test]
    fn long_edge_scales_down_only() {
        assert_eq!(scaled((7008, 4672), Some(1600)), Ok((1600, 1067)));
        assert_eq!(scaled((4672, 7008), Some(1600)), Ok((1067, 1600)));
        assert_eq!(scaled((7008, 4672), None), Ok((7008, 4672)));
        assert_eq!(scaled((7008, 4672), Some(9000)), Ok((7008, 4672)));
        assert_eq!(scaled((7008, 10), Some(0)), Ok((1, 1)));
    }

    #[test]
    fn pixel_cap() {
        assert_eq!(scaled((20_000, 20_000), None), Err(Error::TooLarge { width: 20_000, height: 20_000 }));
        assert!(scaled((20_000, 20_000), Some(4000)).is_ok());
    }
}
