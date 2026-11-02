//! The system's reduce-motion setting: with it on, the preview turns pages without sliding.
//!
//! macOS keeps it in the `com.apple.universalaccess` defaults (System Settings ▸ Accessibility ▸
//! Display ▸ Reduce motion). It is read once when the preview opens. `LIGHTCRAFT_REDUCE_MOTION`
//! (`1` or `0`) overrides it, for tests and for a machine where the setting can't be read; where the
//! setting can't be read at all, motion stays on.

/// Is reduce-motion on?
pub(super) fn reduced() -> bool {
    match std::env::var("LIGHTCRAFT_REDUCE_MOTION") {
        Ok(v) if matches!(v.trim(), "1" | "true" | "yes") => return true,
        Ok(v) if matches!(v.trim(), "0" | "false" | "no") => return false,
        _ => {}
    }
    system()
}

#[cfg(target_os = "macos")]
fn system() -> bool {
    std::process::Command::new("/usr/bin/defaults")
        .args(["read", "com.apple.universalaccess", "reduceMotion"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| parse(&String::from_utf8_lossy(&o.stdout)))
}

#[cfg(not(target_os = "macos"))]
fn system() -> bool {
    false
}

/// What `defaults read` prints for a boolean: `1` or `0`.
#[cfg(any(target_os = "macos", test))]
fn parse(out: &str) -> bool {
    matches!(out.trim(), "1" | "true" | "YES")
}

#[cfg(test)]
mod tests {
    #[test]
    fn defaults_output_is_read() {
        assert!(super::parse("1\n"));
        assert!(!super::parse("0\n"));
        assert!(!super::parse(""));
    }
}
