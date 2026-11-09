//! The fork's own catalog format name (fork note `fork/catalog-format.md`).
//!
//! Official builds and the fork both used to write `"format":"lightcraft-catalog"` with one version counter, so
//! the same number came to mean two formats: the fork's 12 was local camera profiles, the official 12 is shared
//! segmentation grids. From format [`VERSION`] 1 the fork writes snapshots under its own name, [`FORK_FORMAT`],
//! with its own counter:
//! - an official build refuses a fork snapshot untouched (it reads only `format` and `version`, and refuses a
//!   `format` it doesn't know before reading anything else);
//! - the fork never takes an official snapshot for one of its own.
//!
//! What this build reads ([`check`]):
//!
//! | Format | Version | Written by | Read as |
//! |---|---|---|---|
//! | `lightcraft-catalog-fork` | 1 ..= [`VERSION`] | the fork from 2026-11-09 | the fork's format of that number |
//! | `lightcraft-catalog` | 0 ..= 11 | official and fork builds alike (shared history) | older format, upgraded |
//! | `lightcraft-catalog` | 12 | official builds (shared grids) or fork builds (local camera profiles) | older format, upgraded |
//! | `lightcraft-catalog` | 13, 14 | fork builds only (camera profiles, then edited camera values) | older format, upgraded |
//! | `lightcraft-catalog` | 15 and up | a newer official build | refused, untouched |
//!
//! Why 12, 13 and 14 need no guess: a snapshot is read field by field, by name, and every field any of those
//! formats wrote is known to this build under the name it was written with. The two meanings of 12 use different
//! fields (`grids` and `SegMask.grid`; `profile.camera_colour = local | macos` and `profile.camera_profile`), so a
//! file of either, or one holding both, reads exactly. The official counter is at [`OFFICIAL_VERSION`] (12); no
//! official build has written 13 or 14.
//!
//! **When the official format passes 12** (an official update changes its `VERSION` in `journal.rs`, which
//! clashes with the fork's line there): raise [`OFFICIAL_VERSION`], read the official fields, and decide
//! `lightcraft-catalog` 13 and 14 by content, refusing what is still unclear rather than guessing — an official 13
//! read as the fork's would drop the official fields this build doesn't know.

use crate::CatalogError;

/// The format name this build writes.
pub const FORK_FORMAT: &str = "lightcraft-catalog-fork";
/// The official format name: read, never written.
pub const OFFICIAL_FORMAT: &str = "lightcraft-catalog";
/// The fork format this build writes (and the newest it reads). Bump it whenever an `Op` variant or a serialized
/// field is added, and add a row to `journal.rs` → *Format versions*.
pub const VERSION: u32 = 9;
/// The newest official format this build reads (official update of 2026-11-08: shared segmentation grids).
pub const OFFICIAL_VERSION: u32 = 12;
/// The highest `lightcraft-catalog` version a fork build wrote, before the fork had its own name.
pub const LEGACY_FORK_MAX: u32 = 14;

/// How a snapshot header reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Header {
    /// This build's format: nothing to upgrade.
    Current,
    /// An older format this build reads and upgrades: `fork` is whether it is a fork format version.
    Older { fork: bool, version: u32 },
}

/// Checks a snapshot's `format` and `version` before anything else of it is read (see the module docs).
pub fn check(format: &str, version: u32) -> Result<Header, CatalogError> {
    match format {
        FORK_FORMAT if version == VERSION => Ok(Header::Current),
        FORK_FORMAT if version > VERSION => Err(newer(format!("catalog.snap is fork catalog format v{version}"))),
        FORK_FORMAT => Ok(Header::Older { fork: true, version }),
        OFFICIAL_FORMAT if version > OFFICIAL_VERSION.max(LEGACY_FORK_MAX) => Err(newer(format!("catalog.snap is catalog format v{version}"))),
        OFFICIAL_FORMAT => Ok(Header::Older { fork: false, version }),
        other => Err(CatalogError::Corrupt(format!("catalog.snap: not a LightCraft catalog (format {other:?})"))),
    }
}

/// The error for a library written by a newer LightCraft.
pub fn newer(what: String) -> CatalogError {
    CatalogError::Newer(format!(
        "{what}; this version reads fork catalog format v{VERSION} and catalog format v{} and older",
        OFFICIAL_VERSION.max(LEGACY_FORK_MAX)
    ))
}

/// A copy of the snapshot as it was before its first upgrade, kept beside it.
pub const BEFORE_UPGRADE: &str = "catalog.snap.before-upgrade";

#[cfg(test)]
#[path = "tests_fork_format.rs"]
mod tests;
