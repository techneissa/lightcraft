//! The local camera profiles folder, listed at most every [`EVERY`]: the Profile menu asks on each
//! frame it is open, and each listing reads the folder and looks at every file in it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lightcraft_engine::camera_profiles::LocalProfile;

/// How long one listing is reused (a profile added in the Finder shows within it).
const EVERY: Duration = Duration::from_secs(2);

/// The folder, when it was listed and what it held.
type Listing = (Option<PathBuf>, Instant, Vec<Arc<LocalProfile>>);

thread_local! {
    // per thread: the UI has one, and each test its own folder
    static LISTING: std::cell::RefCell<Option<Listing>> = const { std::cell::RefCell::new(None) };
}

/// [`lightcraft_engine::camera_profiles::local_profiles`], listed again only when the folder
/// changed or the last listing is [`EVERY`] old.
pub(super) fn local_profiles() -> Vec<Arc<LocalProfile>> {
    listed(Instant::now(), lightcraft_engine::camera_profiles::local_profiles)
}

fn listed(now: Instant, list: impl FnOnce() -> Vec<Arc<LocalProfile>>) -> Vec<Arc<LocalProfile>> {
    let dir = lightcraft_engine::camera_profiles::dir();
    LISTING.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some((d, at, profiles)) = slot.as_ref()
            && *d == dir
            && now.saturating_duration_since(*at) < EVERY
        {
            return profiles.clone();
        }
        let profiles = list();
        *slot = Some((dir, now, profiles.clone()));
        profiles
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames of an open Profile menu share one listing; one that is old lists the folder again.
    #[test]
    fn the_profiles_folder_is_listed_once_per_two_seconds() {
        let dir = std::env::temp_dir().join(format!("lc-ui-profile-listing-{}", std::process::id()));
        let _dir = lightcraft_engine::camera_profiles::use_dir_for_tests(&dir);
        let runs = std::cell::Cell::new(0);
        let list = || {
            runs.set(runs.get() + 1);
            Vec::new()
        };
        let t0 = Instant::now();
        for frame in 0..60 {
            listed(t0 + Duration::from_millis(16 * frame), list);
        }
        assert_eq!(runs.get(), 1, "one listing for a second of frames");
        listed(t0 + EVERY, list);
        assert_eq!(runs.get(), 2, "listed again once it is old");
    }
}
