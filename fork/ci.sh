#!/bin/sh
# Fork CI's gate (.github/workflows/fork-ci.yml): the steps of `cargo xtask ci` (`cmd_ci` in xtask/src/ci.rs), split
# over ten machines. The official files stay untouched. The tests that fork/known-failures.txt accepts are
# skipped by cargo's test runner on Linux (.cargo/config.toml, fork/skip-known-failures.sh), so a red run is a new
# failure.
#
# A pull request tests only the crates it can affect (fork/affected.py: the changed crates and every crate that
# depends on them); a push to `custom` tests the whole workspace. The slowest crate's tests are split over the last
# groups, one shard each (SHARDS in fork/affected.py, run by fork/skip-known-failures.sh), and the other crates over
# the first groups by about equal test time; group 1 also runs fmt, clippy, layers, assets, parity and the wasm check. The tests run with
# `cargo test`, not nextest: the demo library's scenes are cached per process, and nextest regenerates them for
# every test (much slower for the ui-egui tests).
#
#   fork/ci.sh --part 1/10        group 1 of ten: the one-off steps and its share of the tests (the default)
#   fork/ci.sh --part 2/10        the second group
#   fork/ci.sh --part 1/10 --print print the commands without running them
#   FORK_CI_FULL=1 fork/ci.sh --part 1/10    the whole workspace on a pull request too
#   FORK_CI_FULL=0 GITHUB_EVENT_NAME=pull_request GITHUB_BASE_REF=custom fork/ci.sh --part 1/10 --print   a pull request
set -eu
cd "$(dirname "$0")/.."

print=false
part="1/10"
while [ $# -gt 0 ]; do
  case "$1" in
    --print) print=true ;;
    --part) shift; part="$1" ;;
    *) echo "fork/ci.sh: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

# What to test: a pull request's affected crates, else everything.
changed=""
full=--full
if [ "${GITHUB_EVENT_NAME:-}" = "pull_request" ] && [ -n "${GITHUB_BASE_REF:-}" ] && [ "${FORK_CI_FULL:-}" != 1 ]; then
  if git fetch --quiet --no-tags --depth=1 origin "$GITHUB_BASE_REF" 2>/dev/null; then
    changed="$(git diff --name-status FETCH_HEAD HEAD)"
    full=""
  fi
fi
# shellcheck disable=SC2086
eval "$(printf '%s\n' "$changed" | python3 -I fork/affected.py $full --sh --part "$part")"
echo "=== fork ci: part $part, scope: $SCOPE${SHARD:+, shards: $SHARD}"
# which shards of a split crate's tests this group runs (read by fork/skip-known-failures.sh)
export FORK_TEST_SHARD="$SHARD"

export RUST_TEST_THREADS="${RUST_TEST_THREADS:-4}"
started=$(date +%s)
failed=""
timings=""
# step <name> <command...>: runs the command, notes a failure and goes on, so one run lists every failing step
step() {
  name="$1"; shift
  echo "=== fork ci: $name"
  echo "\$ $*"
  $print && return
  t0=$(date +%s)
  "$@" || failed="$failed $name"
  timings="$timings
$name: $(( $(date +%s) - t0 )) s"
}

# Unquoted on purpose below: $CLIPPY_ARGS, $PACKAGES and the like are lists of arguments (`-p a -p b`).
# shellcheck disable=SC2086
if [ "$ONEOFF" = 1 ]; then
  step affected-tests python3 -I fork/test_affected.py
  step fmt cargo fmt --all -- --check
  if [ "$RUST" = 1 ]; then
    step clippy cargo clippy $CLIPPY_ARGS --all-targets --keep-going -- -D warnings
    if [ "$HEIF" = 1 ]; then
      step heif-clippy cargo clippy -p lightcraft-codecs --features heif --all-targets -- -D warnings
    fi
    step layers cargo xtask layers
    step assets cargo xtask assets
    if [ "$PARITY" = 1 ]; then step parity cargo xtask parity; fi
    if [ "$WASM" = 1 ]; then step wasm cargo xtask wasm; fi
  fi
fi
# shellcheck disable=SC2086
if [ -n "$PACKAGES" ]; then
  step test cargo test $PACKAGES $FEATURES --lib --bins --tests
fi
# shellcheck disable=SC2086
if [ -n "$DOC_PACKAGES" ]; then
  step doctest cargo test $DOC_PACKAGES $DOC_FEATURES --doc
fi
if [ "$HEIF_TESTS" = 1 ]; then
  step heif-test cargo test -p lightcraft-codecs -p lightcraft-heif --features lightcraft-codecs/heif --lib --bins --tests
  step heif-engine-test cargo test -p lightcraft-engine --lib --features lightcraft-codecs/heif heic
fi
# The GPU tests (`gpu-backends` in cargo xtask ci) need a hardware adapter; the runners have none.

$print && exit 0
total=$(( $(date +%s) - started ))
summary="Fork CI part $part: $total s (scope: $SCOPE)$timings"
echo "$summary"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  { echo '### Fork CI'; echo '```'; echo "$summary"; echo '```'; } >> "$GITHUB_STEP_SUMMARY"
fi
if [ -n "$failed" ]; then
  echo "::error::Fork CI part $part failed at:$failed"
  exit 1
fi
echo "Fork CI part $part: every step passed"
