//! Fork feature (`fork/retouch-hair.md`, "Stray Hair v2"): the strands outside the hair, taken away once per photo by the
//! face search.
//!
//! Next to Blemishes' spots and Stray Hair v1's strands, on the search's worker: for each face, on a full-size crop of
//! the head, Apple Vision's person segmentation (Accurate) gives the matte, the strands are what is thin and long in it
//! outside the hair mass, and LaMa fills them; the change that results is kept in `Strands::removal`
//! (`lightcraft_retouch::hair::with_removals`). A render only scales it with the Amount slider; no model runs while one
//! moves. A row in the activity stack (`retouch.strands`) shows the search, and its ✕ stops it: the faces then keep no
//! change and Stray Hair v1 looks for their strands.
//!
//! Without Apple Vision (not a Mac, or older than macOS 14) or without the LaMa model, nothing here runs and Stray Hair
//! is v1 alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_color::transfer::encode_srgb8;
use lightcraft_coreml::vision::{self, Image, Pixels, Request};
use lightcraft_develop::retouch::Face;
use lightcraft_develop::retouch::hair::{Fill, PhotoRect, Remover};
use lightcraft_pipeline::learned_fill::FillRun;
use lightcraft_raster::{Plane, Rgb32f};

use crate::activity::{Activity, Cancel};
use crate::lama_fill::LamaFill;

/// What a face search needs to take strands away: the LaMa fill (whether or not the model is there) and the activity
/// stack for the search's row.
#[derive(Clone)]
pub(crate) struct StrandRemoval {
    pub lama: LamaFill,
    pub activity: Activity,
}

/// Whether the search can run at all here: Apple Vision's person segmentation and the LaMa model.
pub(crate) fn possible(lama: &LamaFill) -> bool {
    vision::supported(Request::PersonSegmentation) && lama.installed()
}

impl StrandRemoval {
    /// Whether the search can run (see [`possible`]).
    pub(crate) fn available(&self) -> bool {
        possible(&self.lama)
    }

    /// `faces` with the strands outside their hair taken away, on crops of the head the photo (`photo`, its size turned
    /// like the masks) is cut into by `crop`.
    pub(crate) fn run(&self, faces: Vec<Face>, photo: (usize, usize), crop: &mut dyn FnMut(PhotoRect) -> Option<Rgb32f>) -> Vec<Face> {
        self.run_with(faces, photo, crop, |lama, run, stop| Box::new(Tools { lama, run, stop, matte: person_matte }))
    }

    /// [`StrandRemoval::run`] with the tools `make` builds (tests put their own in to look at the row and stop it).
    fn run_with(
        &self,
        faces: Vec<Face>,
        photo: (usize, usize),
        crop: &mut dyn FnMut(PhotoRect) -> Option<Rgb32f>,
        make: impl for<'b> FnOnce(&'b LamaFill, &'b FillRun, &'b AtomicBool) -> Box<dyn Remover + 'b>,
    ) -> Vec<Face> {
        let stop = Arc::new(AtomicBool::new(false));
        let run = FillRun::with_stop(stop.clone());
        // (the row is held by the run, so LaMa's own row is not added to it)
        run.hold(Box::new(self.activity.start("retouch.strands", "Removing Stray Hair", Cancel::Flag(stop.clone()))));
        let mut tools = make(&self.lama, &run, &stop);
        lightcraft_develop::retouch::hair::with_removals(photo, faces, crop, tools.as_mut())
    }
}

/// Apple Vision and LaMa as the search's [`Remover`].
struct Tools<'a> {
    lama: &'a LamaFill,
    run: &'a FillRun,
    stop: &'a AtomicBool,
    /// The person matte of a crop (Apple Vision's; a test puts its own).
    matte: fn(&Rgb32f) -> Option<Plane>,
}

impl Remover for Tools<'_> {
    fn matte(&mut self, crop: &Rgb32f) -> Option<Plane> {
        if self.cancelled() {
            return None;
        }
        (self.matte)(crop)
    }

    fn fill(&mut self, rgb: &Rgb32f, hole: &Plane) -> Fill {
        if self.cancelled() {
            return Fill::Cancelled;
        }
        if !self.lama.installed() {
            return Fill::Unavailable;
        }
        let flat: Vec<f32> = rgb.data.iter().flatten().copied().collect();
        match self.lama.fill_strands(self.run, &flat, &hole.data, rgb.width, rgb.height) {
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

/// Apple Vision's person segmentation (Accurate) of the linear picture `crop`, as a coverage plane the crop's aspect;
/// `None` when Vision can't make one (not available, a damaged picture, an error or a panic: logged).
fn person_matte(crop: &Rgb32f) -> Option<Plane> {
    let rgb: Vec<u8> = crop.data.iter().flat_map(|p| p.map(encode_srgb8)).collect();
    let image = Image { width: crop.width, height: crop.height, pixels: Pixels::Rgb8(&rgb) };
    let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| vision::person_segmentation(&image)));
    match made {
        Ok(Ok(m)) if m.is_valid() => Some(Plane { width: m.width, height: m.height, data: m.data }),
        Ok(Ok(_)) => None,
        Ok(Err(e)) => {
            log::warn!("stray hair: Apple Vision: {e}");
            None
        }
        Err(_) => {
            log::warn!("stray hair: Apple Vision stopped unexpectedly");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use lightcraft_develop::retouch::{Face, NormRect, Part, PartMask};

    use super::*;

    const PHOTO: (usize, usize) = (600, 600);

    /// A face with a Hair mask (the head's), on a 600 × 600 photo.
    fn face() -> Face {
        let r = NormRect::new(0.3, 0.3, 0.7, 0.7);
        Face { masks: vec![(Part::Hair, PartMask { plane: Plane::filled(8, 8, 1.0), area: NormRect::new(0.25, 0.2, 0.75, 0.8) })], ..Face::boxed(r) }
    }

    fn search() -> StrandRemoval {
        let activity = Activity::default();
        StrandRemoval { lama: LamaFill::new(activity.clone()), activity }
    }

    /// A 600 × 600 photo's crops: flat grey.
    fn crop(rect: PhotoRect) -> Option<Rgb32f> {
        Some(Rgb32f::filled(rect[2] - rect[0], rect[3] - rect[1], [0.4; 3]))
    }

    /// With no LaMa model there is nothing to search with: the search is not available, whatever Vision does.
    #[test]
    fn without_the_model_the_search_is_not_available() {
        assert!(!search().available());
        assert!(!possible(&search().lama));
    }

    /// While it runs there is one row, `retouch.strands`, that can be cancelled; cancelling it keeps no change; the row
    /// is gone when the search ends.
    #[test]
    fn the_search_shows_a_row_and_cancel_stops_it() {
        struct Stopper<'a>(Activity, Arc<std::sync::atomic::AtomicUsize>, &'a AtomicBool);
        impl Remover for Stopper<'_> {
            fn matte(&mut self, _: &Rgb32f) -> Option<Plane> {
                let rows = self.0.list();
                self.1.store(rows.iter().filter(|r| r.kind == "retouch.strands").count(), Ordering::SeqCst);
                for r in &rows {
                    self.0.cancel(r.id).expect("the row can be cancelled");
                }
                None
            }

            fn fill(&mut self, _: &Rgb32f, _: &Plane) -> Fill {
                Fill::Unavailable
            }

            fn cancelled(&self) -> bool {
                self.2.load(Ordering::Relaxed)
            }
        }
        let s = search();
        let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (activity, seen2) = (s.activity.clone(), seen.clone());
        fn stopper<'b>(a: Activity, seen: Arc<std::sync::atomic::AtomicUsize>, stop: &'b AtomicBool) -> Box<dyn Remover + 'b> {
            Box::new(Stopper(a, seen, stop))
        }
        // (the stop flag is what the row's ✕ sets)
        let faces = s.run_with(vec![face()], PHOTO, &mut |r| crop(r), move |_, _, stop| stopper(activity, seen2, stop));
        assert_eq!(seen.load(Ordering::SeqCst), 1, "one row while the search ran");
        assert!(faces[0].strands.is_none(), "a face the search did not finish keeps nothing");
        assert!(s.activity.list().is_empty(), "the row ends with the search");
    }

    /// A matte, a fill and no strand in it: the face is searched and keeps an empty change (v1 then looks at its skin only).
    #[test]
    fn a_face_without_strands_keeps_an_empty_change() {
        struct Flat;
        impl Remover for Flat {
            fn matte(&mut self, c: &Rgb32f) -> Option<Plane> {
                // everyone is the middle square: a solid head and nothing thin
                Some(Plane::from_fn(c.width, c.height, |x, y| {
                    let (u, v) = (x as f32 / c.width as f32, y as f32 / c.height as f32);
                    if (0.25..0.75).contains(&u) && (0.25..0.75).contains(&v) { 1.0 } else { 0.0 }
                }))
            }

            fn fill(&mut self, _: &Rgb32f, _: &Plane) -> Fill {
                Fill::Unavailable
            }
        }
        let s = search();
        let faces = s.run_with(vec![face()], PHOTO, &mut |r| crop(r), |_, _, _| Box::new(Flat));
        let strands = faces[0].strands.as_ref().expect("searched");
        assert!(strands.removal.as_ref().is_some_and(|r| r.is_empty()), "an empty change, kept");
        assert_eq!(strands.count, 0);
    }
}
