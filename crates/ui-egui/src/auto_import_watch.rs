//! What the Auto Import poll remembers between polls, so an idle watched folder is not listed every
//! few seconds: the folder is listed when its modification time changed, while a file is still being
//! confirmed, and at least once a minute (a subfolder's change doesn't bump its parent's time).
use std::time::SystemTime;

/// Seconds between full listings, whatever the folder's time says.
pub const FULL_LISTING_SECS: f64 = 60.0;

#[derive(Clone, Debug, Default)]
pub struct Watch {
    folder: String,
    /// the folder's modification time at the last poll
    stamp: Option<SystemTime>,
    /// when the folder was last listed (monotonic app time)
    listed_at: f64,
    /// the last scan left a file to confirm (new, or its size still changing): list again at once
    waiting: bool,
}

impl Watch {
    /// What the next poll of `folder` passes to `poll_auto_import_folder`: the time to compare with, and
    /// whether to list regardless.
    pub fn ask(&self, folder: &str, now: f64) -> (Option<SystemTime>, bool) {
        if self.folder != folder {
            return (None, true);
        }
        (self.stamp, self.waiting || now - self.listed_at >= FULL_LISTING_SECS)
    }

    /// A poll of `folder` came back at `now` (`listed`: with its files).
    pub fn polled(&mut self, folder: &str, stamp: Option<SystemTime>, listed: bool, now: f64) {
        if self.folder != folder {
            self.folder = folder.to_string();
            self.waiting = false;
        }
        self.stamp = stamp;
        if listed {
            self.listed_at = now;
        }
    }

    /// The scan of a listing said whether files are left to confirm (`waiting` in its result).
    pub fn scanned(&mut self, waiting: bool) {
        self.waiting = waiting;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A quiet folder is listed once a minute and when its time moves, not every poll; a file being
    /// confirmed and a changed folder name bring the listing back at once.
    #[test]
    fn an_idle_folder_is_not_listed_every_poll() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let mut w = Watch::default();
        assert_eq!(w.ask("/watch", 100.0), (None, true), "the first poll lists");
        w.polled("/watch", Some(t), true, 100.0);
        w.scanned(false);
        assert_eq!(w.ask("/watch", 103.0), (Some(t), false), "idle: the time is compared, nothing listed");
        assert_eq!(w.ask("/watch", 159.0), (Some(t), false));
        assert_eq!(w.ask("/watch", 160.0), (Some(t), true), "the safety net, once a minute");
        w.polled("/watch", Some(t), true, 160.0);
        assert_eq!(w.ask("/watch", 163.0), (Some(t), false));
        w.scanned(true);
        assert_eq!(w.ask("/watch", 163.0), (Some(t), true), "a file to confirm: list again");
        w.scanned(false);
        assert_eq!(w.ask("/other", 164.0), (None, true), "another folder starts over");
    }
}
