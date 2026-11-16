//! Fork (`fork/retouch-background.md`): a photo's depth map, the auxiliary depth image an iPhone Portrait photo
//! carries next to its picture. Portrait ▸ Background ▸ Blur reads it to blur what is far more than what is near.

use heic_rs::context::Context;
use heic_rs::meta::iref::RefKind;
use heic_rs::props::Transform;
use heic_rs::props::simple::AuxKind;

use crate::{Error, Options, decode_item, err, guarded, item_visits, orient, probe_unguarded};

/// A depth map as stored, 0…1 (an iPhone stores disparity: larger is nearer), in the picture's orientation.
#[derive(Clone, Debug, PartialEq)]
pub struct DepthMap {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// Whether an auxiliary type names a depth or disparity image.
fn is_depth(kind: AuxKind, urn: &str) -> bool {
    kind == AuxKind::Depth || urn.contains(":aux:depth") || urn.contains(":aux:disparity")
}

/// The depth map of the file's picture, when it has one; `Ok(None)` when it has none.
pub fn decode_depth(bytes: &[u8], options: &Options) -> Result<Option<DepthMap>, Error> {
    guarded(|| {
        probe_unguarded(bytes)?;
        let ctx = Context::open(bytes).map_err(err)?;
        let primary = ctx.meta.primary;
        let found = ctx
            .meta
            .refs
            .iter()
            .filter(|r| r.kind == RefKind::Auxl && r.to.contains(&primary))
            .map(|r| r.from)
            .find(|id| ctx.props(*id).ok().and_then(|p| p.auxc).is_some_and(|a| is_depth(a.kind, a.urn)));
        let Some(aux) = found else { return Ok(None) };
        let mut visits = 0_usize;
        item_visits(&ctx, aux, 0, &mut visits)?;
        let d = decode_item(&ctx, aux, options, false)?;
        let (w, h) = (d.width as usize, d.height as usize);
        let mut img = orient::Pixels { width: w, height: h, ch: 3, data: d.data };
        if options.apply_transforms {
            // the picture's rotation and mirror (its crop is in its own pixels, and a depth image carries none of
            // these itself)
            let p = ctx.props(primary).map_err(err)?;
            let turn: Vec<Transform> = p.transforms.iter().filter(|t| !matches!(t, Transform::Crop(_))).cloned().collect();
            img = orient::apply(img, &turn)?;
        }
        let max = f32::from(u16::MAX);
        let data = img.data.chunks(3).map(|p| f32::from(p.first().copied().unwrap_or(0)) / max).collect();
        Ok(Some(DepthMap { width: img.width, height: img.height, data }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata::{Spec, build};

    #[test]
    fn a_depth_image_is_read_and_a_photo_without_one_has_none() {
        let sample = |x: u32, y: u32| [(x * 4) as u16, (y * 4) as u16, 128];
        let mut spec = Spec::new(64, 48, &sample);
        let plain = build(&spec);
        assert_eq!(decode_depth(&plain, &Options::default()), Ok(None));
        spec.ignored_aux = vec!["urn:mpeg:hevc:2015:auxid:2"];
        let with = build(&spec);
        let d = decode_depth(&with, &Options::default()).unwrap().unwrap();
        assert_eq!((d.width, d.height), (64, 48));
        // (the builder's depth image is (x + y) mod 256)
        let at = |x: usize, y: usize| d.data[y * d.width + x];
        assert!(at(40, 40) > at(2, 2) + 0.2, "{} vs {}", at(40, 40), at(2, 2));
        assert!(d.data.iter().all(|v| (0.0..=1.0).contains(v)));
        // the picture itself still decodes as before
        assert!(crate::decode(&with, &Options::default()).is_ok());
        assert!(decode_depth(b"not a heif file", &Options::default()).is_err());
    }
}
