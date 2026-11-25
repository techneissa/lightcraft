#!/usr/bin/env python3
"""Do two versions of Fork CI test the same things? Compares BASE (default mine/custom) with this checkout.

    python3 -I fork/coverage.py [BASE]

For each version it reads `fork/ci.sh --part N/10 --print` of all ten parts (a full run) and lists what runs: the
one-off and heif commands, each crate's tests and doc tests with the crate's own features, and the shards of a split
crate. Then it runs that version's test runner (fork/skip-known-failures.sh) on a stand-in test binary, laid out as
cargo lays one out, once per shard, and counts how often each test is started. Exits 1 unless both versions list the
same things and this checkout starts every test of a split crate exactly once. No build; `cargo metadata` only.
"""

import collections
import io
import os
import re
import shlex
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# where cargo puts a test binary: Rust 1.100 and later (what the runners use), as seen in the logs
BINARY = "target/debug/build/{crate}/0123456789abcdef/out/stand_in-0123456789abcdef"
TESTS = [f"module::test_{i:03}" for i in range(100)]


def parts(tree):
    """[(part, shard spec, [command])] of a full run, as `tree`'s fork/ci.sh prints it."""
    wf = (tree / ".github/workflows/fork-ci.yml").read_text(encoding="utf-8")
    total = int(re.search(r"--part \$\{\{ matrix\.part \}\}/(\d+)", wf).group(1))
    env = {k: v for k, v in os.environ.items() if k not in ("GITHUB_EVENT_NAME", "GITHUB_BASE_REF")}
    out = []
    for n in range(1, total + 1):
        text = subprocess.run(
            ["sh", "fork/ci.sh", "--part", f"{n}/{total}", "--print"],
            cwd=tree, env=env | {"FORK_CI_FULL": "1"}, capture_output=True, text=True, check=True,
        ).stdout
        shard = re.search(r"^=== fork ci: part .*, shards: (.*)$", text, re.M)
        # the sharded step is a check of the runner itself, not of the app
        commands = [line[2:] for line in text.splitlines() if line.startswith("$ ") and not line.startswith("$ test -s ")]
        out.append((n, shard.group(1) if shard else "", commands))
    return out


def inventory(plan):
    """What a run tests, as {item: times}; a split crate counts once when every one of its shards runs once."""
    items = collections.Counter()
    shards = collections.defaultdict(list)
    for _, spec, commands in plan:
        for s in spec.split():
            crate, _, which = s.partition("=")
            index, _, count = which.partition("/")
            shards[crate, int(count)] += [int(i) for i in index.split(",")]
        for command in commands:
            args = shlex.split(command)
            kind = "doc tests" if args[-1] == "--doc" else "tests" if args[-3:] == ["--lib", "--bins", "--tests"] else None
            if args[:2] != ["cargo", "test"] or kind is None or "lightcraft-codecs/heif" in command:
                items["step: " + command] += 1
                continue
            features = args[args.index("--features") + 1].split(",") if "--features" in args else []
            for crate in (args[i + 1] for i, a in enumerate(args) if a == "-p"):
                own = ",".join(f.partition("/")[2] for f in features if f.startswith(crate + "/"))
                items[f"{kind}: {crate} [{own}]"] += 1
    for (crate, count), seen in shards.items():
        whole = sorted(seen) == list(range(1, count + 1))
        for item in list(items):
            if item.startswith(f"tests: {crate} ["):
                items[item] = 1 if whole and items[item] == len(set(seen)) else items[item]
    return items


def times_started(tree, plan):
    """{crate: Counter of how often the runner starts each of TESTS over all parts} for the split crates."""
    started = {}
    for _, spec, _ in plan:
        for s in spec.split():
            crate = s.partition("=")[0]
            with tempfile.TemporaryDirectory() as tmp:
                binary = Path(tmp) / BINARY.format(crate=crate)
                binary.parent.mkdir(parents=True)
                listing = "".join(f"{t}: test\\n" for t in TESTS)
                binary.write_text(f'#!/bin/sh\nif [ "$1" = --list ]; then printf \'{listing}\'; else printf \'%s\\n\' "$@"; fi\n')
                binary.chmod(0o755)
                env = os.environ | {"FORK_TEST_SHARD": spec, "CARGO_PKG_NAME": crate, "FORK_SHARD_MARK": ""}
                args = subprocess.run(
                    ["bash", str(tree / "fork/skip-known-failures.sh"), str(binary)],
                    env=env, capture_output=True, text=True, check=True,
                ).stdout.splitlines()
            # no --exact: the runner passed the binary through, and it runs everything
            run = [a for a in args if a in TESTS] if "--exact" in args else TESTS
            started.setdefault(crate, collections.Counter()).update(run)
    return started


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else "mine/custom"
    with tempfile.TemporaryDirectory() as tmp:
        archive = subprocess.run(["git", "archive", base], cwd=ROOT, capture_output=True, check=True).stdout
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(tmp, filter="data")
        results = {}
        for label, tree in (("before", Path(tmp)), ("after", ROOT)):
            plan = parts(tree)
            results[label] = (plan, inventory(plan), times_started(tree, plan))
    ok = True
    for label, (plan, items, started) in results.items():
        print(f"{label} ({base if label == 'before' else 'this checkout'}): {len(plan)} parts")
        for kind in ("tests", "doc tests", "step"):
            print(f"  {kind}: {sum(1 for i in items if i.startswith(kind + ':'))}")
        for crate, count in started.items():
            shards = sum(1 for _, spec, _ in plan if spec.startswith(crate + "="))
            print(f"  {crate} in {shards} shards: each test starts {sorted(set(count.values()))} time(s), {len(count)}/{len(TESTS)} covered")
    before, after = results["before"][1], results["after"][1]
    if before != after:
        ok = False
        for item in sorted(before.keys() | after.keys()):
            if before[item] != after[item]:
                print(f"DIFFERS: {item}: {before[item]} before, {after[item]} after")
    twice = sorted(i for i, n in after.items() if n != 1)
    if twice:
        ok = False
        print("RUNS MORE THAN ONCE:", *twice, sep="\n  ")
    for crate, count in results["after"][2].items():
        if len(count) != len(TESTS) or set(count.values()) != {1}:
            ok = False
            print(f"{crate}: its tests do not each start exactly once")
    print("--- the list (equal before and after)" if before == after else "--- the list after")
    for item in sorted(after):
        print("  " + item)
    print("coverage: equal, each exactly once" if ok else "coverage: NOT equal")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
