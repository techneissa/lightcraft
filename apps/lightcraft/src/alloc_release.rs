//! Give memory the system allocator keeps after frees back to the system
//! ([`lightcraft_engine::memory::set_release_hook`]). The platform call lives in
//! `lightcraft-sysmem`, the workspace's only crate allowed `unsafe`.

/// Install the hook.
pub fn install() {
    lightcraft_engine::memory::set_release_hook(release);
}

fn release() {
    let _ = lightcraft_sysmem::release_free_memory();
}

/// macOS 26 and later: freed large blocks stay in the process's memory (up to 2 GB: every raw decode's pixels)
/// and `malloc_zone_pressure_relief` returns none of it. Starting with `MallocLargeCache=0`, which libmalloc reads
/// only at process start, hands them back at once. So the first start replaces itself with the same program and
/// arguments and that variable (same process, same window-server identity; no unsafe); the second start sees the
/// variable and goes on. `LIGHTCRAFT_KEEP_MALLOC_CACHE=1` keeps the allocator as it is (issue #304).
#[cfg(target_os = "macos")]
pub fn restart_without_large_cache() {
    use std::os::unix::process::CommandExt;
    if std::env::var_os("MallocLargeCache").is_some() || std::env::var_os("LIGHTCRAFT_KEEP_MALLOC_CACHE").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let mut args = std::env::args_os();
    let mut command = std::process::Command::new(exe);
    if let Some(name) = args.next() {
        command.arg0(name);
    }
    // (only returns when the program could not be started: this one goes on as it is)
    let _ = command.args(args).env("MallocLargeCache", "0").exec();
}

#[cfg(not(target_os = "macos"))]
pub fn restart_without_large_cache() {}
