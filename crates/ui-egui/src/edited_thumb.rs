//! The open photo's thumbnail waits out a burst of spot edits (issue #97).
//!
//! Every edit renders the photo's filmstrip thumbnail again, a second full render next to the loupe's. While the Remove
//! panel is open, the thumbnail of the photo in the loupe now follows once its edits have paused for [`SETTLE`]
//! seconds and the loupe has its render: a burst of spots renders it once, at the end, after the loupe's. A slider drag already holds it back the same way
//! (`panels::grid::request_thumb`).

use lightcraft_catalog::PhotoId;

use crate::LightcraftApp;
use crate::state::RightPanel;

/// Seconds without an edit before the open photo's thumbnail renders again, while the Remove panel is open.
pub(crate) const SETTLE: f64 = 0.4;

/// The look of the open photo the thumbnail was last held for, since when, and whether the wait for it is over.
#[derive(Default)]
pub(crate) struct Hold {
    held: Option<Held>,
}

#[derive(Clone, Copy)]
struct Held {
    photo: PhotoId,
    look: usize,
    since: f64,
    /// Waited out: this look's thumbnail renders, and is not held again.
    over: bool,
}

impl Hold {
    /// Seconds left before the held thumbnail may render (`None`: nothing is waiting, the window need not wake).
    pub(crate) fn left(&self, now: f64) -> Option<f64> {
        self.held.filter(|h| !h.over).map(|h| h.since + SETTLE - now).filter(|left| *left > 0.0)
    }

    /// Whether a thumbnail of `photo` at `look` (`None`: not the open photo's, which never waits and leaves the hold
    /// as it is) waits at `now`; `busy`: the loupe is still rendering.
    fn waits(&mut self, photo: PhotoId, look: Option<usize>, now: f64, busy: bool) -> bool {
        let Some(look) = look else { return false };
        match self.held {
            Some(h) if h.photo == photo && h.look == look => {
                if h.over {
                    return false;
                }
                // (and while the loupe renders: the thumbnail never takes the device from it)
                if now - h.since < SETTLE || busy {
                    return true;
                }
                self.held = Some(Held { over: true, ..h });
                false
            }
            // a new look: wait from now (the first frame that asks for it)
            _ => {
                self.held = Some(Held { photo, look, since: now, over: false });
                true
            }
        }
    }
}

/// Whether `id`'s thumbnail, which is not of the photo's current look, waits now: the Remove panel is open on it, it
/// already has a thumbnail, and its settings changed less than [`SETTLE`] seconds ago.
pub(crate) fn waits(app: &mut LightcraftApp, id: PhotoId) -> bool {
    let open = app.ui.right == RightPanel::Remove && app.session.active() == Some(id) && app.renderer.thumb(id).is_some();
    let look = app.session.catalog.photo(id).filter(|_| open).map(|p| std::sync::Arc::as_ptr(&p.develop) as usize);
    let (now, busy) = (app.tick_now(), app.renderer.is_pending(crate::render::Slot::Main));
    app.edited_thumb.waits(id, look, now, busy)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: PhotoId = PhotoId(1);

    #[test]
    fn nothing_is_left_once_the_wait_is_over() {
        let mut h = Hold::default();
        assert!(h.waits(OPEN, Some(7), 0.0, false));
        assert!(h.left(0.1).is_some_and(|l| (l - 0.3).abs() < 1e-9));
        // (it used to stay Some(0.0), and the window redrew every 20 ms for good)
        assert_eq!(h.left(0.5), None);
    }

    #[test]
    fn a_look_waited_out_is_not_held_again() {
        let mut h = Hold::default();
        assert!(h.waits(OPEN, Some(7), 0.0, false));
        assert!(!h.waits(OPEN, Some(7), 0.5, false), "renders once the edits pause");
        assert!(!h.waits(OPEN, Some(7), 0.6, false), "and is not held again for the same look");
        assert_eq!(h.left(0.6), None);
        assert!(h.waits(OPEN, Some(8), 0.7, false), "a new edit is held again");
    }

    #[test]
    fn another_photos_thumbnail_leaves_the_open_photos_hold() {
        let mut h = Hold::default();
        assert!(h.waits(OPEN, Some(7), 0.0, false));
        // an unedited raw next to it in the filmstrip asks for its thumbnail every frame
        assert!(!h.waits(PhotoId(2), None, 0.2, false));
        assert!(!h.waits(OPEN, Some(7), 0.5, false), "the open photo's wait still ends");
    }

    #[test]
    fn the_thumbnail_waits_for_the_loupe() {
        let mut h = Hold::default();
        assert!(h.waits(OPEN, Some(7), 0.0, true));
        assert!(h.waits(OPEN, Some(7), 0.5, true));
        assert!(!h.waits(OPEN, Some(7), 0.6, false));
    }
}
