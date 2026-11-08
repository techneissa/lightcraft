//! The open photo's thumbnail waits out a burst of spot edits (issue #97).
//!
//! Every edit renders the photo's filmstrip thumbnail again, a second full render next to the loupe's. While the Remove
//! panel is open, the thumbnail of the photo in the loupe now follows once its edits have paused for [`SETTLE`]
//! seconds: a burst of spots renders it once, at the end. A slider drag already holds it back the same way
//! (`panels::grid::request_thumb`).

use lightcraft_catalog::PhotoId;

use crate::LightcraftApp;
use crate::state::RightPanel;

/// Seconds without an edit before the open photo's thumbnail renders again, while the Remove panel is open.
pub(crate) const SETTLE: f64 = 0.4;

/// The look of the open photo the thumbnail was last held for, and since when.
#[derive(Default)]
pub(crate) struct Hold {
    held: Option<(PhotoId, usize, f64)>,
}

impl Hold {
    /// Seconds left before the held thumbnail may render (`None`: nothing is held).
    pub(crate) fn left(&self, now: f64) -> Option<f64> {
        self.held.map(|(.., since)| (since + SETTLE - now).max(0.0))
    }
}

/// Whether `id`'s thumbnail, which is not of the photo's current look, waits now: the Remove panel is open on it, it
/// already has a thumbnail, and its settings changed less than [`SETTLE`] seconds ago.
pub(crate) fn waits(app: &mut LightcraftApp, id: PhotoId) -> bool {
    let open = app.ui.right == RightPanel::Remove && app.session.active() == Some(id) && app.renderer.thumb(id).is_some();
    let Some(look) = app.session.catalog.photo(id).filter(|_| open).map(|p| std::sync::Arc::as_ptr(&p.develop) as usize) else {
        app.edited_thumb.held = None;
        return false;
    };
    let now = app.tick_now();
    match app.edited_thumb.held {
        Some((p, l, since)) if p == id && l == look => {
            if now - since < SETTLE {
                return true;
            }
            app.edited_thumb.held = None;
            false
        }
        // a new look: wait from now (the first frame that asks for it)
        _ => {
            app.edited_thumb.held = Some((id, look, now));
            true
        }
    }
}
