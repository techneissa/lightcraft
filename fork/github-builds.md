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

Pull requests to `custom` are tested before they merge, and that costs no extra minutes from this file: the official
`CI` workflow (`ci.yml`) already has a `pull_request` trigger with no branch filter, so every PR to `custom` runs
`cargo xtask ci` on the PR's merge with `custom` (about 30 to 40 minutes on Linux; the repo is public, so GitHub
bills nothing for it). Fork CI does not trigger on `pull_request` as well: it only dispatches `ci.yml`, so it would
start a second identical run for every push to a PR branch (checked 2026-11-02 on `gh run list -w CI`: every
`feat/…` PR already has its `pull_request` run). Both runs skip the tests in `known-failures.txt`
([known-failures.md](known-failures.md)): the skip is cargo's test runner on Linux (`.cargo/config.toml`), so it
applies to every `cargo test` on Linux, the PR runs and the dispatched ones alike, and no official workflow file is
edited. A red run means a new failure.

A daily agent (the T3 thread "Daily LightCraft Update", Grok 4.5 on Cursor, 09:00) runs the update without
installing; when GitHub's run stopped on a clash, it merges by hand, runs the feature tests and pushes, and GitHub
builds again. It follows the "Daily run" section of the `craftupdate` skill. To stop it: switch it off under
Scheduled Tasks in T3, or ask any agent to "turn off the daily LightCraft update".

The official project's own workflows (CI, Nix, Packaging lint, SonarCloud) are switched off in this fork:
the official project runs them. A workflow the official project adds later starts switched on; switch it off
in Actions unless it is wanted here.
