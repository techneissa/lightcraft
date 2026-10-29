//! `photo.deleteRejected`: send the rejected photos of the view on screen to Recently Deleted.

use super::selection::{mark_deleted, select_after_deletion};
use super::{CommandSpec, Flag, LibrarySource, Result, Session, Value, bad, cmd, json};

/// Not inside Recently Deleted, where the photos are already deleted. (Whether the view holds a
/// rejected photo is the menu's business: a call with none returns `{changed: 0}`.)
fn not_in_recently_deleted(s: &Session) -> std::result::Result<(), String> {
    if s.source == LibrarySource::RecentlyDeleted { Err("these photos are already in Recently Deleted".into()) } else { Ok(()) }
}

impl Session {
    /// The rejected photos the view shows, in view order.
    pub fn rejected_in_view(&mut self) -> Vec<lightcraft_catalog::PhotoId> {
        let view = self.visible_cloned();
        view.into_iter().filter(|id| self.is_rejected(*id)).collect()
    }

    /// Whether the view as last drawn shows a rejected photo (a menu asks this every frame, so it
    /// reads the list the grid already made instead of working the view out again).
    pub fn has_rejected_in_view(&self) -> bool {
        self.visible.iter().any(|id| self.is_rejected(*id))
    }

    fn is_rejected(&self, id: lightcraft_catalog::PhotoId) -> bool {
        self.catalog.photo(id).is_some_and(|p| p.flag == Flag::Reject && !p.deleted)
    }
}

fn delete_rejected(s: &mut Session, _p: &Value) -> Result<Value> {
    if s.source == LibrarySource::RecentlyDeleted {
        return Err(bad("photo.deleteRejected", "these photos are already in Recently Deleted"));
    }
    let before = s.visible_cloned();
    let targets = s.rejected_in_view();
    if targets.is_empty() {
        return Ok(json!({"changed": 0}));
    }
    // an open drag ends first, as its own undo step (the selection moves on)
    s.end_interaction()?;
    let n = mark_deleted(s, &targets, "Delete Rejected Photos")?;
    select_after_deletion(s, &before);
    Ok(json!({"changed": n}))
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "photo.deleteRejected",
        "Delete Rejected Photos",
        ["Photo"],
        Some("Cmd+Delete"),
        "{} → {changed} — moves the rejected photos of the view on screen (the folder, album or source shown, as filtered) to Recently Deleted, as one undo step; rejected photos elsewhere are left alone; {changed: 0} when the view holds none; refused inside Recently Deleted",
        not_in_recently_deleted,
        delete_rejected
    )]
}
