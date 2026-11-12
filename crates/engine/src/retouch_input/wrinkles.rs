//! Fork feature (`fork/retouch-wrinkles.md`): the Wrinkles search of the face search.
//!
//! Next to Blemishes' spots and Stray Hair's strands, once per photo on the face search's worker: for each face with the
//! face mesh and a Facial Skin mask, the lines found and the change that evens the skin out across them, kept in
//! `Face::wrinkles` (`lightcraft_retouch::wrinkles::with_fixes_from`; the filler's LaMa fill is no longer called). A
//! render only scales it with the sliders; no model runs while one moves. A row in the activity stack
//! (`retouch.wrinkles`) shows the search, and its ✕ stops it: no face then keeps a change, Wrinkles shows its old
//! result, and the photo's faces are kept as "to search again" (`Found::retry`).
//!
//! The pixels are floating-point crops of the original with the default settings, rendered at the size the search works
//! at ([`Cropper::crop_float`]); the 8-bit parse picture is the fallback for a photo the original can't be cut for.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_develop::retouch::wrinkles::{Crop, Fill, Filler, Picture, Source};
use lightcraft_develop::retouch::{Face, NormRect, Part};
use lightcraft_pipeline::learned_fill::FillRun;
use lightcraft_raster::{Plane, Rgb32f};

use super::parts::Cropper;
use crate::activity::{Activity, Cancel};
use crate::lama_fill::LamaFill;

/// What a face search needs to look for wrinkles: the LaMa fill (whether or not the model is there) and the activity
/// stack for the search's row.
#[derive(Clone)]
pub(crate) struct WrinkleSearch {
    pub lama: LamaFill,
    pub activity: Activity,
    /// Cuts float crops of the photo's original (`None`: the parse's 8-bit picture is used).
    pub cropper: Option<Arc<Cropper>>,
    /// Tests: the search is stopped before it starts, as the row's ✕ would.
    #[cfg(test)]
    pub cancel_first: bool,
}

/// The face mesh's point count and YuNet's five before it: a face with fewer has no areas to search.
const MESH_LANDMARKS: usize = 5 + 478;

impl WrinkleSearch {
    /// A search with no float crops and not cancelled (the engine's own setup adds the cropper).
    pub(crate) fn new(lama: LamaFill, activity: Activity, cropper: Option<Arc<Cropper>>) -> Self {
        WrinkleSearch {
            lama,
            activity,
            cropper,
            #[cfg(test)]
            cancel_first: false,
        }
    }

    /// `faces` with their wrinkle change found, on float crops of the original or else on `linear` (the parse's picture
    /// in linear light, turned like the masks). Faces the search can't use are left as they are. The flag says the search
    /// was cancelled: no face then has a change.
    pub(crate) fn run(&self, faces: Vec<Face>, linear: &Rgb32f) -> (Vec<Face>, bool) {
        self.run_with(faces, linear, |lama, run, stop| Box::new(LamaFiller { lama, run, stop }))
    }

    /// [`WrinkleSearch::run`] with the filler `make` builds (tests put their own in to look at the row and stop it).
    fn run_with(
        &self,
        faces: Vec<Face>,
        linear: &Rgb32f,
        make: impl for<'b> FnOnce(&'b LamaFill, &'b FillRun, &'b AtomicBool) -> Box<dyn Filler + 'b>,
    ) -> (Vec<Face>, bool) {
        let any = faces.iter().any(|f| f.landmarks.len() >= MESH_LANDMARKS && f.mask(Part::FacialSkin).is_some());
        if !any {
            return (faces, false);
        }
        #[cfg(test)]
        let first = self.cancel_first;
        #[cfg(not(test))]
        let first = false;
        let stop = Arc::new(AtomicBool::new(first));
        let run = FillRun::with_stop(stop.clone());
        // (the row is held by the run, so LaMa's own row is not added to it)
        run.hold(Box::new(self.activity.start_background("retouch.wrinkles", "Smoothing Wrinkles", Cancel::Flag(stop.clone()))));
        let mut filler = make(&self.lama, &run, &stop);
        let source = FloatSource { cropper: self.cropper.as_deref(), picture: Picture(linear) };
        let faces = lightcraft_develop::retouch::wrinkles::with_fixes_from(&source, faces, filler.as_mut());
        let cancelled = stop.load(Ordering::Relaxed);
        (faces, cancelled)
    }
}

/// The search's pixels: float crops of the original when there is a cutter and it can, else the 8-bit parse picture.
struct FloatSource<'a> {
    cropper: Option<&'a Cropper>,
    picture: Picture<'a>,
}

impl Source for FloatSource<'_> {
    fn size(&self) -> (usize, usize) {
        self.cropper.map_or_else(|| self.picture.size(), Cropper::photo_size)
    }

    fn crop(&self, rect: NormRect, max_side: usize) -> Option<Crop> {
        self.cropper.and_then(|c| c.crop_float(rect, max_side)).or_else(|| self.picture.crop(rect, max_side))
    }
}

/// LaMa as the search's [`Filler`].
struct LamaFiller<'a> {
    lama: &'a LamaFill,
    run: &'a FillRun,
    stop: &'a AtomicBool,
}

impl Filler for LamaFiller<'_> {
    fn fill(&mut self, rgb: &Rgb32f, hole: &Plane) -> Fill {
        if self.cancelled() {
            return Fill::Cancelled;
        }
        if !self.lama.installed() {
            return Fill::Unavailable;
        }
        let flat: Vec<f32> = rgb.data.iter().flatten().copied().collect();
        match self.lama.fill_wrinkles(self.run, &flat, &hole.data, rgb.width, rgb.height) {
            Some(out) if out.len() == flat.len() => {
                let data = out.as_chunks::<3>().0.to_vec();
                Fill::Done(Rgb32f { width: rgb.width, height: rgb.height, data })
            }
            _ if self.cancelled() => Fill::Cancelled,
            _ => Fill::Unavailable,
        }
    }

    fn cancelled(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || lightcraft_gpu::shutting_down()
    }
}

#[cfg(test)]
mod tests {
    use lightcraft_develop::retouch::wrinkles::areas;
    use lightcraft_develop::retouch::{NormRect, PartMask};

    use super::*;

    const SIZE: (usize, usize) = (320, 400);

    /// A face with the mesh and a Facial Skin mask, and a picture with two dark lines across its forehead.
    fn face_and_picture() -> (Face, Rgb32f) {
        let [x0, y0, w, h] = [60.0 / 320.0, 40.0 / 400.0, 200.0 / 320.0, 200.0 / 400.0];
        let at = |(fx, fy): (f64, f64)| (x0 + fx * w, y0 + fy * h);
        let five = [(0.33, 0.45), (0.67, 0.45), (0.5, 0.62), (0.38, 0.78), (0.62, 0.78)];
        let face = Face {
            landmarks: five.into_iter().map(at).chain((0..areas::MESH).map(|i| at(areas::frontal(i)))).collect(),
            masks: vec![(Part::FacialSkin, PartMask { plane: Plane::filled(8, 8, 1.0), area: NormRect::new(x0, y0 - 0.1 * h, x0 + w, y0 + h) })],
            ..Face::boxed(NormRect::new(x0, y0, x0 + w, y0 + h))
        };
        let img = Rgb32f::from_fn(SIZE.0, SIZE.1, |_, y| {
            let line = |c: f32| (-0.5 * ((y as f32 - c) / 1.4).powi(2)).exp();
            let k = 1.0 - 0.3 * line(64.0).max(line(76.0));
            [0.45 * k, 0.3 * k, 0.24 * k]
        });
        (face, img)
    }

    fn search() -> WrinkleSearch {
        let activity = Activity::default();
        WrinkleSearch::new(LamaFill::new(activity.clone()), activity, None)
    }

    /// Without the model the banded heal makes the change, under one row that is gone when the search ends.
    #[test]
    fn the_search_without_the_model_heals_and_leaves_no_row() {
        let (face, img) = face_and_picture();
        let s = search();
        assert!(!s.lama.installed());
        let (faces, cancelled) = s.run(vec![face], &img);
        assert!(!cancelled);
        assert!(faces[0].wrinkles.as_ref().is_some_and(|f| !f.is_empty()), "the banded heal's change is kept");
        assert!(s.activity.list().is_empty(), "the row ends with the search");
    }

    /// While it runs there is one row, `retouch.wrinkles`, that can be cancelled; cancelling it keeps no change.
    #[test]
    fn the_search_shows_a_row_and_cancel_stops_it() {
        struct Stopper<'a>(Activity, Arc<std::sync::atomic::AtomicUsize>, &'a AtomicBool);
        impl Filler for Stopper<'_> {
            fn fill(&mut self, _: &Rgb32f, _: &Plane) -> Fill {
                Fill::Unavailable
            }

            // (the search no longer fills with LaMa (#201), so the row is checked and its ✕ pressed the first time the
            // search asks whether it should stop)
            fn cancelled(&self) -> bool {
                if self.1.load(Ordering::SeqCst) == 0 {
                    let rows = self.0.list();
                    self.1.store(rows.len(), Ordering::SeqCst);
                    assert_eq!(rows.iter().filter(|r| r.kind == "retouch.wrinkles").count(), 1, "{rows:?}");
                    for r in &rows {
                        self.0.cancel(r.id).expect("the row can be cancelled");
                    }
                }
                self.2.load(Ordering::Relaxed)
            }
        }
        let (face, img) = face_and_picture();
        let s = search();
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        fn stopper<'b>(a: Activity, seen: Arc<std::sync::atomic::AtomicUsize>, stop: &'b AtomicBool) -> Box<dyn Filler + 'b> {
            Box::new(Stopper(a, seen, stop))
        }
        // (the stop flag is what the row's ✕ sets)
        let (activity, seen2) = (s.activity.clone(), seen.clone());
        let (faces, cancelled) = s.run_with(vec![face], &img, move |_, _, stop| stopper(activity, seen2, stop));
        assert!(cancelled, "the search says it was cancelled");
        assert_eq!(seen.load(Ordering::SeqCst), 1, "one row while the search ran");
        assert_eq!(faces[0].wrinkles, None, "a cancelled search keeps nothing: Wrinkles shows its old result");
        assert!(s.activity.list().is_empty());
    }

    /// A photo whose faces have no mesh or no skin mask starts no search and no row.
    #[test]
    fn faces_the_search_cannot_use_are_left_alone() {
        let (mut face, img) = face_and_picture();
        face.landmarks.truncate(5);
        let s = search();
        let (faces, cancelled) = s.run(vec![face.clone()], &img);
        assert_eq!(faces, vec![face]);
        assert!(!cancelled);
    }
}
