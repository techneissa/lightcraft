#!/usr/bin/env bash
# Cargo's test runner on Linux (.cargo/config.toml, `[target.'cfg(target_os = "linux")']`): runs each test binary
# with `--skip <test>` for every line of fork/known-failures.txt. Only test binaries get the extra arguments: cargo
# keeps them in `target/<profile>/build/<crate>/<hash>/out/` (Rust 1.100 and later) or `target/<profile>/deps/`
# (earlier); anything else cargo starts through the runner (`cargo xtask` is a `cargo run` of `target/debug/xtask`)
# runs unchanged. Doc tests do not go through a runner and are not skipped.
#
# Shards (fork/affected.py SHARDS): FORK_TEST_SHARD="crate=2,6/6 other=1/3" runs only shards 2 and 6 of 6 of each
# test binary of `crate` (cargo names the binary's package in CARGO_PKG_NAME). A binary's tests are listed, sorted,
# and test number i (from 0) goes to shard i % 6 + 1; the known failures are dropped, and the rest run by exact name.
# Each sharded binary is noted in the file FORK_SHARD_MARK: fork/ci.sh fails a group that has a shard and no note.
set -u
bin="$1"
shift
case "$bin" in
  */deps/* | */build/*/out/*)
    list="$(dirname "${BASH_SOURCE[0]}")/known-failures.txt"
    known=()
    if [ -f "$list" ]; then
      while IFS= read -r line; do
        line="${line%%#*}"
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        [ -n "$line" ] && known+=("$line")
      done < "$list"
    fi
    shard=""
    for spec in ${FORK_TEST_SHARD:-}; do
      [ "${spec%%=*}" = "${CARGO_PKG_NAME:-}" ] && shard="${spec#*=}"
    done
    if [ -n "$shard" ]; then
      count="${shard#*/}"
      picked=",${shard%/*},"
      names=()
      i=0
      while IFS= read -r name; do
        mine=false
        case "$picked" in *",$(( i % count + 1 )),"*) mine=true ;; esac
        i=$(( i + 1 ))
        $mine || continue
        for k in ${known[@]+"${known[@]}"}; do
          case "$name" in *"$k"*) continue 2 ;; esac
        done
        names+=("$name")
      done < <("$bin" --list --format terse | sed -n 's/: test$//p' | LC_ALL=C sort)
      echo "fork ci: $(basename "$bin"): shard $shard runs ${#names[@]} of $i tests"
      [ -n "${FORK_SHARD_MARK:-}" ] && echo "$(basename "$bin") $shard" >> "$FORK_SHARD_MARK"
      # no test of this shard in the binary: running it with no names would run all of them
      [ ${#names[@]} -eq 0 ] && exit 0
      exec "$bin" "$@" --exact "${names[@]}"
    fi
    skips=()
    for k in ${known[@]+"${known[@]}"}; do skips+=(--skip "$k"); done
    exec "$bin" "$@" ${skips[@]+"${skips[@]}"}
    ;;
  *) exec "$bin" "$@" ;;
esac
