# GitHub builds the app

The workflow `.github/workflows/fork-build.yml` (Actions ▸ Fork build) runs daily, on every push to `custom`, and
by hand. It merges the official update into `custom` the same way the script does, builds the Mac app, and
publishes it as the release [`custom-build`](https://github.com/techneissa/lightcraft/releases/tag/custom-build)
(its notes name the commit). The update script downloads it ([update.md](update.md)). A run that hits a merge clash fails and
changes nothing; GitHub emails the failure. The run pushes its merge with the secret `FORK_PUSH_TOKEN`, the
fine-grained token "BuildCraft forks" (the LightCraft, PdfCraft and SolidCraft forks; Contents and Workflows:
read and write): GitHub's own token may not push an official update that changes a workflow file. Without the
secret it uses GitHub's own token.

The workflow `.github/workflows/fork-ci.yml` (Actions ▸ Fork CI) runs the official CI (`ci.yml`, `cargo xtask ci` on
Linux) on `custom` after every push to it, the daily merge included, so the full suite runs on GitHub and not on
this Mac. The official `CI` workflow must stay enabled on the fork for it (`gh workflow enable CI -R
techneissa/lightcraft`).

## Pull requests: `fork-gate`

A pull request to `custom` waits for one check: **`fork-gate`** (workflow `.github/workflows/fork-pr.yml`, Actions ▸
Fork PR). It tests what the pull request can break, on three machines at once, instead of the whole workspace on
one (the official `ci.yml` took 35 to 55 minutes per PR). The jobs:

- `plan`: `fork/pr_plan.py` lists the crates the PR changes, plus every workspace crate that depends on them
  (`cargo metadata`, normal, dev and build dependencies). A change to `Cargo.lock`, the root `Cargo.toml`,
  `.cargo/`, `.config/`, `xtask/`, `.github/`, `clippy.toml`, `rustfmt.toml`, `fork/known-failures.txt`,
  `fork/skip-known-failures.sh` or the plan script itself means every crate. A file outside every crate (notes,
  `fork/`, docs, packaging) changes no crate, except the files a crate compiles in (`EMBEDDED` in
  `xtask/src/ci_quick.rs`, whose test keeps that list complete; `fork/test_pr_plan.py` checks the script's copy).
  The script also splits the crates into three groups of about equal test time (a crate's weight: its tests plus
  its lines of Rust over 200; `WEIGHT_OVERRIDES` in the script holds measured values).
  Run it on this Mac: `python3 fork/pr_plan.py --base mine/custom`; its tests: `python3 fork/test_pr_plan.py`.
- `quick`: `cargo fmt --all --check`; clippy `-D warnings --all-targets` on the planned crates (and the `heif`
  feature when `lightcraft-codecs` or `lightcraft-heif` is planned); `cargo xtask layers` and `assets`;
  `cargo xtask parity` when the parity tracker, a command or control crate (`lightcraft-engine`, `-develop`,
  `-ui-egui`, `-cli`) or a deleted or renamed file is in the change; `cargo xtask wasm` when a planned crate is in
  the web build.
- `tests (1/3)`, `(2/3)`, `(3/3)`: `cargo test -p …` for the group's crates (lib, bins and integration tests, then
  doc tests), plus the `heif` tests in one group when planned. Same apt packages, craft-fonts commit and Rust
  install as `ci.yml`; a Rust cache per job. They go through the test runner in `.cargo/config.toml`, so the
  accepted failures in `known-failures.txt` ([known-failures.md](known-failures.md)) are skipped as in Fork CI.
  Each group writes its time to the run's summary.
- `fork-gate`: passes only when `plan`, `quick` and every test group passed. A failing test or clippy warning in
  a planned crate fails it. Its name, `fork-gate`, is what merges wait for.

A pull request with only notes (Markdown, `fork/` files) tests no crate: `quick` still checks format, layers,
assets and (for the tracker docs) parity. A newer push to the PR cancels the older run.

A push to `custom` (the daily merge included) is unchanged: Fork CI runs the full official `cargo xtask ci`.
Pull requests into `main` keep the official `CI` too.

### Hook in an official file

`.github/workflows/ci.yml` has one added line under `pull_request:`: `branches-ignore: [custom]`, so the official
check no longer runs on pull requests into `custom` (`fork-gate` replaces it). When an official update changes
that `on:` block and the merge clashes, keep the line by hand. Without it both checks run on every PR.

Both runs skip the tests in `known-failures.txt`: the skip is cargo's test runner on Linux (`.cargo/config.toml`),
so it applies to every `cargo test` on Linux, and no official workflow file is edited. A red run means a new
failure.

A daily agent (the T3 thread "Daily LightCraft Update", Grok 4.5 on Cursor, 09:00) runs the update without
installing; when GitHub's run stopped on a clash, it merges by hand, runs the feature tests and pushes, and GitHub
builds again. It follows the "Daily run" section of the `craftupdate` skill. To stop it: switch it off under
Scheduled Tasks in T3, or ask any agent to "turn off the daily LightCraft update".

The official project's own workflows (CI, Nix, Packaging lint, SonarCloud) are switched off in this fork:
the official project runs them. A workflow the official project adds later starts switched on; switch it off
in Actions unless it is wanted here.
