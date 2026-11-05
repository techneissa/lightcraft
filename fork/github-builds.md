# GitHub builds the app

The workflow `.github/workflows/fork-build.yml` (Actions ▸ Fork build) runs daily, on every push to `custom`, and
by hand. It merges the official update into `custom` the same way the script does, builds the Mac app, and
publishes it as the release [`custom-build`](https://github.com/techneissa/lightcraft/releases/tag/custom-build)
(its notes name the commit). The update script downloads it ([update.md](update.md)). A run that hits a merge clash fails and
changes nothing; GitHub emails the failure. The run pushes its merge with the secret `FORK_PUSH_TOKEN`, the
fine-grained token "BuildCraft forks" (the LightCraft, PdfCraft and SolidCraft forks; Contents and Workflows:
read and write): GitHub's own token may not push an official update that changes a workflow file. Without the
secret it uses GitHub's own token.

## Fork CI and `fork-gate`

The workflow `.github/workflows/fork-ci.yml` (Actions ▸ Fork CI) is the check on GitHub, so the full suite never runs
on this Mac. It calls techneissa's copy of the shared gate, `techneissa/craft-common-actions` (storytold's plus an `sccache`
input, on here, that also caches the workspace crates' compiled code; merged from storytold weekly, see FORK.md there). One workflow for both events, through the suite's shared gate workflow (the same Ubuntu packages,
craft-fonts commit, Rust install and cache as `ci.yml`; it reads the packages and the commit from `ci.yml`), running
`fork/ci.sh` as ten jobs at once (`gate (1)` to `gate (10)`):

- **A pull request to `custom`** no longer starts Fork CI (techneissa, 2026-11-03: a batch of PRs is merged and tested once); run by hand on a branch, it tests what it can break. `fork/affected.py` lists the crates with a changed file
  plus every workspace crate that depends on one (`cargo metadata`; normal, dev and build dependencies).
  `Cargo.toml`, `Cargo.lock`, `.cargo/`, `.config/`, `.github/`, `xtask/`, `clippy.toml`, `rustfmt.toml`,
  `fork/known-failures.txt`, `fork/skip-known-failures.sh`, `fork/ci.sh`, `fork/affected.py` or any file outside the
  crates that is not Markdown (it may be a build input) tests every crate. Exceptions: a file a crate compiles in
  counts for that crate (`EMBEDDED`, copied from `xtask/src/ci_quick.rs`, whose test keeps it complete), and
  `fork/tests.d/` is no build input. A change to notes only (Markdown outside the crates) runs the format check and
  nothing else.
- **A push to `custom`** (the daily upstream merge included) tests everything: the steps of `cargo xtask ci`
  (format, clippy, the `heif` feature, tests and doc tests, parity, layers, assets, wasm) except the GPU hardware
  run, which needs an adapter the runners lack.
- **The ten jobs** each run `cargo test -p …` for a group of crates, balanced by an estimated time per crate
  (seconds on a runner: its tests plus its lines of Rust over 200, times 0.1; `WEIGHT_OVERRIDES`, `ONEOFF_WEIGHT` and
  `HEIF_WEIGHT` in `fork/affected.py` hold measured values). `lightcraft-ui-egui`'s tests take about 24 minutes
  (2438 lib tests), so they are split over the last five jobs (`SHARDS`), one shard each and nothing else: each runs
  every fifth test of each of the crate's test binaries, by exact name (`FORK_TEST_SHARD`, read by
  `fork/skip-known-failures.sh`), and the first also runs its doc tests. The other crates share the first five jobs.
  Compiling is most of a job's time (6 to 8 minutes, and every shard compiles the crate), so a crate next to a shard
  makes that job the slowest, and more shards buy little.
  The number of jobs is `GROUPS` in `fork/affected.py` and the matrix in `fork-ci.yml`; a test keeps them equal.
  Group 1 also runs the one-off steps: format, clippy `-D warnings --all-targets` on the tested
  crates, `cargo xtask layers` and `assets`, `parity` when the tracker docs, a command or control crate
  (`lightcraft-engine`, `-develop`, `-ui-egui`, `-cli`) or a deleted or renamed file is in the change, `wasm` when a
  tested crate is in the web build. Every test run passes the features the whole workspace turns on (for example
  `lightcraft-engine/sam`, from the desktop app): `cargo test -p …` alone builds fewer and fails the tests that need
  them. It is `cargo test`, not nextest: the demo scenes are cached per process and nextest generates them again for
  every test (`ci.yml` notes it is much slower here). Each job writes its time to the run's summary.
- **`fork-gate`** is the last job and passes only when every job passed. Its name is what a pull request waits
  for. A failing test or clippy warning in a tested crate fails it.
- The accepted failures in `known-failures.txt` ([known-failures.md](known-failures.md)) are skipped by cargo's test
  runner on Linux (`.cargo/config.toml`), so a red run is a new failure.

On this Mac: `git diff --name-only mine/custom | python3 -I fork/affected.py --summary` shows the plan,
`sh fork/ci.sh --part 1/10 --print` the commands, `python3 fork/test_affected.py` the script's tests. Nothing else of
`fork/ci.sh` runs here. A run by hand (Actions ▸ Fork CI ▸ Run workflow) on `custom` also saves the dependency cache
that pull requests then restore; other runs only read it.

### The official `CI` workflow is switched off

The official `CI` workflow (`ci.yml`, one machine, the whole suite, 35 to 55 minutes) is disabled in the repo's
Actions settings, not by a file edit, so `ci.yml` stays as the official project has it and an official update never
clashes on it. It would run a second full check on every pull request and push.

- Turn it off: `gh workflow disable CI -R techneissa/lightcraft` (Actions ▸ CI ▸ ⋯ ▸ Disable workflow).
- Turn it back on: `gh workflow enable CI -R techneissa/lightcraft`.
- Fork CI reads two values from `ci.yml` (the Ubuntu packages and the craft-fonts commit), so it keeps working while
  `CI` is disabled; it fails with a message when `ci.yml` no longer has them.

A daily agent (the T3 thread "Daily LightCraft Update", Grok 4.5 on Cursor, 09:00) runs the update without
installing; when GitHub's run stopped on a clash, it merges by hand, runs the feature tests and pushes, and GitHub
builds again. It follows the "Daily run" section of the `craftupdate` skill. To stop it: switch it off under
Scheduled Tasks in T3, or ask any agent to "turn off the daily LightCraft update".

The official project's own workflows (CI, Nix, Packaging lint, SonarCloud) are switched off in this fork:
the official project runs them. A workflow the official project adds later starts switched on; switch it off
in Actions unless it is wanted here.
