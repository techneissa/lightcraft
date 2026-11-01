#!/usr/bin/env bash
# Cargo's test runner on Linux (.cargo/config.toml, `[target.'cfg(target_os = "linux")']`): runs each test binary
# with `--skip <test>` for every line of fork/known-failures.txt. Only binaries in a `deps/` folder (the test
# binaries) get the extra arguments; anything else cargo starts through the runner (`cargo xtask` is a
# `cargo run`) runs unchanged. Doc tests do not go through a runner and are not skipped.
set -u
bin="$1"
shift
case "$bin" in
  */deps/*)
    list="$(dirname "${BASH_SOURCE[0]}")/known-failures.txt"
    skips=()
    if [ -f "$list" ]; then
      while IFS= read -r line; do
        line="${line%%#*}"
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        [ -n "$line" ] && skips+=(--skip "$line")
      done < "$list"
    fi
    exec "$bin" "$@" ${skips[@]+"${skips[@]}"}
    ;;
  *) exec "$bin" "$@" ;;
esac
