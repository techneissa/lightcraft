# Checks for a fork feature

Check in batches:
- after each change, and once more at the end of the round: only the tests of the code it touched
  (`cargo test -p <crate> <filter>`), `cargo clippy -p <crate> --all-targets -- -D warnings` for the crates it
  touched, and `cargo fmt --all --check`;
- the full suite runs on GitHub, not on this Mac: a push to `custom` starts Fork CI (`fork-ci.yml`), which runs the
  official CI (`cargo xtask ci`) on `custom`. Check it with `gh run list -R techneissa/lightcraft -w CI -b custom`
  and fix a failure in the next round.

This fork reads `CLAUDE.md`'s "before every commit" and "run the full one locally before merging" as the above:
no `cargo xtask ci` or `ci --quick` on this Mac (techneissa, 2026-10-29: the same tests ran three times per
feature and loaded the Mac). The official CI runs on Linux, so macOS-only tests (`macraw`, macOS Colour) run
only in a lane's own filtered tests. A failure Fork CI shows only on Linux is noted, not fixed (Mac only, in FORK.md).
