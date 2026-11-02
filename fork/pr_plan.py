#!/usr/bin/env python3
"""Plan the checks of a pull request to `custom` (.github/workflows/fork-pr.yml).

A pull request tests what it can break: the workspace crates with a changed file, plus every workspace crate
that depends on one of them (normal, dev and build dependencies, read from `cargo metadata --no-deps`). A
change to a workspace-wide file tests every crate. The test groups are balanced for three machines.

    python3 fork/pr_plan.py --base HEAD^1                 # the PR merge commit against its base
    python3 fork/pr_plan.py --base mine/custom            # a local branch against custom
    python3 fork/pr_plan.py --files names.txt             # one changed path per line (an earlier PR's list)

Prints the plan as JSON on stdout and a summary on stderr. With --github-output it also appends the plan's
fields to $GITHUB_OUTPUT. Tests: `python3 fork/test_pr_plan.py`. Why and how: fork/github-builds.md.
"""

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# A change to one of these tests every crate. `xtask/` and `.github/` are folders; the others are exact paths.
WORKSPACE_WIDE_FILES = {
    "Cargo.toml",
    "Cargo.lock",
    "clippy.toml",
    "rustfmt.toml",
    ".rustfmt.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
    "fork/known-failures.txt",
    "fork/skip-known-failures.sh",
    "fork/pr_plan.py",
    "fork/test_pr_plan.py",
}
WORKSPACE_WIDE_DIRS = (".cargo/", ".config/", ".github/", "xtask/")

# Files outside a crate's folder that the crate compiles in. Mirrors EMBEDDED in xtask/src/ci_quick.rs, whose test
# `embedded_files_are_mapped` keeps it complete, so any other file outside every crate cannot change a build.
# test_pr_plan.py checks that this table and the Rust one agree.
EMBEDDED = [
    ("assets/app-icon/", ["lightcraft", "lightcraft-ui-egui"]),
    ("assets/camera-profiles/", ["lightcraft-engine"]),
    ("assets/camera-tone/", ["lightcraft-engine"]),
    ("assets/fonts/", ["lightcraft-engine", "lightcraft-ui-egui"]),
    ("contributors/", ["lightcraft-ui-egui"]),
    ("docs/whats-new.md", ["lightcraft-ui-egui"]),
    ("docs/raw/coverage-sweep.tsv", ["lightcraft-raw"]),
    ("docs/raf-corpus.sha256", ["lightcraft-raw"]),
    ("xtask/src/raw_samples.rs", ["lightcraft-raw"]),
    ("crates/ui-egui/src/menus.rs", ["xtask"]),
    ("crates/ui-egui/src/menus/", ["xtask"]),
]

# `cargo xtask parity` reads these crates' command and control ids and the tracker docs; a deleted or renamed file
# can break a path the tracker cites. Same rule as `ci --quick` (xtask/src/ci_quick.rs).
PARITY_PACKAGES = {"lightcraft-engine", "lightcraft-develop", "lightcraft-ui-egui", "lightcraft-cli"}
PARITY_DOCS = {"docs/parity-checklist.md", "docs/gaps.md"}
HEIF_PACKAGES = {"lightcraft-codecs", "lightcraft-heif"}

# Test time per crate is guessed from its size: a test counts 1, 200 lines of Rust count 1. A crate whose
# measured test time (a run's step summary) shows a group out of balance goes here, in the same unit.
WEIGHT_OVERRIDES = {}
# The heif extras (clippy and tests with the feature) count as a crate for the balance.
HEIF_WEIGHT = 120

TEST_ATTR = re.compile(r"#\[(?:\w+::)*test\b")


def run(cmd, cwd=ROOT):
    out = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, check=False)
    if out.returncode != 0:
        sys.exit(f"{' '.join(cmd)}: {out.stderr.strip()}")
    return out.stdout


def load_metadata(path=None):
    text = Path(path).read_text(encoding="utf-8") if path else run(["cargo", "metadata", "--no-deps", "--format-version", "1"])
    return json.loads(text)


def workspace_packages(meta, root=ROOT):
    """The workspace crates: name, folder relative to the root, workspace dependencies, doc-test support."""
    root = Path(meta.get("workspace_root") or root)
    members = set(meta["workspace_members"])
    pkgs = {}
    for p in meta["packages"]:
        if p["id"] not in members:
            continue
        folder = Path(p["manifest_path"]).parent
        try:
            rel = folder.relative_to(root).as_posix()
        except ValueError:
            continue
        has_doctests = any(
            t.get("doctest", False) and any(k in ("lib", "rlib", "proc-macro") for k in t["kind"]) for t in p["targets"]
        )
        pkgs[p["name"]] = {"dir": rel, "deps": {d["name"] for d in p["dependencies"]}, "doc": has_doctests}
    for p in pkgs.values():
        p["deps"] &= pkgs.keys()
    return pkgs


def parse_name_status(out):
    """`git diff --name-status -z -M` output to [(path, gone)]; a renamed-from path is gone."""
    fields = out.split("\0")
    changed, i = [], 0
    while i < len(fields):
        status = fields[i].strip()
        i += 1
        if not status or i >= len(fields):
            continue
        kind = status[0]
        first = fields[i]
        i += 1
        if kind in "RC":
            changed.append((first, kind == "R"))
            if i < len(fields):
                changed.append((fields[i], False))
                i += 1
        else:
            changed.append((first, kind == "D"))
    return changed


def changed_from_git(base, root=ROOT):
    merge_base = run(["git", "merge-base", base, "HEAD"], root).strip()
    return parse_name_status(run(["git", "diff", "--name-status", "-z", "-M", merge_base, "HEAD"], root))


def owner(path, pkgs):
    """The innermost crate folder holding `path`."""
    best = None
    for name, p in pkgs.items():
        d = p["dir"]
        if d and path.startswith(d + "/") and (best is None or len(d) > len(pkgs[best]["dir"])):
            best = name
    return best


def embedders(path):
    for prefix, names in EMBEDDED:
        if path.startswith(prefix) if prefix.endswith("/") else path == prefix:
            yield from names


def is_workspace_wide(path):
    return path in WORKSPACE_WIDE_FILES or path.startswith(WORKSPACE_WIDE_DIRS)


def reverse_closure(changed, pkgs):
    """`changed` plus every crate that depends on one of them, directly or not."""
    planned = set(changed)
    grew = True
    while grew:
        grew = False
        for name, p in pkgs.items():
            if name not in planned and p["deps"] & planned:
                planned.add(name)
                grew = True
    return planned


def wasm_set(pkgs, layers_rs):
    """Crates `cargo xtask wasm` checks for wasm32: layers 0 to 5, standalone crates and lightcraft-web."""
    try:
        text = Path(layers_rs).read_text(encoding="utf-8")
        table = text.split("pub const TABLE", 1)[1].split("];", 1)[0]
    except (OSError, IndexError):
        return set(pkgs)  # unreadable table: check every crate rather than none
    classes = dict(re.findall(r'\("([a-z0-9-]+)",\s*Class::(Layer\(\d+\)|Standalone|Testkit|Exempt)\)', table))
    out = {"lightcraft-web"} & pkgs.keys()
    for name in pkgs:
        cls = classes.get(name.removeprefix("lightcraft-"))
        layer = re.fullmatch(r"Layer\((\d+)\)", cls or "")
        if cls == "Standalone" or (layer and int(layer.group(1)) <= 5):
            out.add(name)
    return out


def weight(name, pkgs, root=ROOT):
    if name in WEIGHT_OVERRIDES:
        return WEIGHT_OVERRIDES[name]
    tests = lines = 0
    for f in (Path(root) / pkgs[name]["dir"]).rglob("*.rs"):
        if "target" in f.parts:
            continue
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        tests += len(TEST_ATTR.findall(text))
        lines += text.count("\n")
    return max(1, tests + lines // 200)


def balance(weights, groups):
    """Longest-first: each item goes to the lightest group. Returns [(total, [names])] with empty groups dropped."""
    bins = [[0, []] for _ in range(groups)]
    for name, w in sorted(weights.items(), key=lambda kv: (-kv[1], kv[0])):
        lightest = min(bins, key=lambda b: (b[0], len(b[1])))
        lightest[0] += w
        lightest[1].append(name)
    return [(total, sorted(names)) for total, names in bins if names]


def plan(changed, pkgs, wasm, groups=3, weights=None):
    """What the checks run. `changed` is [(path, gone)]; `weights` maps crate to test weight (default: measured on disk)."""
    direct, full = set(), False
    parity = heif_files = False
    for path, gone in changed:
        if is_workspace_wide(path):
            full = True
        if path in PARITY_DOCS or gone:
            parity = True
        o = owner(path, pkgs)
        if o:
            direct.add(o)
        for name in embedders(path):
            if name in pkgs:
                direct.add(name)
    if full:
        planned = set(pkgs)
    else:
        planned = reverse_closure(direct, pkgs)
    parity = parity or full or bool(direct & PARITY_PACKAGES)
    heif = full or bool(planned & HEIF_PACKAGES)
    weights = weights or {}
    items = {n: weights[n] if n in weights else weight(n, pkgs) for n in planned}
    if heif:
        items["+heif"] = HEIF_WEIGHT
    bins = balance(items, groups) if items else []
    tests = []
    for number, (total, names) in enumerate(bins, 1):
        crates = [n for n in names if n != "+heif"]
        tests.append(
            {
                "group": number,
                "name": f"{number}/{len(bins)}",
                "packages": " ".join(f"-p {n}" for n in crates),
                "doc_packages": " ".join(f"-p {n}" for n in crates if pkgs[n]["doc"]),
                "heif": "+heif" in names,
                "weight": total,
                "crates": crates,
            }
        )
    return {
        "full": full,
        "rust": bool(planned),
        "changed": sorted(direct),
        "crates": sorted(planned),
        "clippy_args": "--workspace" if full else " ".join(f"-p {n}" for n in sorted(planned)),
        "wasm": bool(planned & wasm),
        "parity": parity,
        "heif": heif,
        "tests": tests,
    }


def summary(result, files):
    lines = [f"{files} changed file(s)"]
    if result["full"]:
        lines.append("a workspace-wide file changed: every crate is tested")
    lines.append(f"changed crates: {', '.join(result['changed']) or 'none'}")
    lines.append(f"tested crates ({len(result['crates'])}): {', '.join(result['crates']) or 'none'}")
    lines.append(
        f"wasm check: {'yes' if result['wasm'] else 'no'}; parity: {'yes' if result['parity'] else 'no'}; heif: {'yes' if result['heif'] else 'no'}"
    )
    for t in result["tests"]:
        lines.append(f"test group {t['name']} (weight {t['weight']}): {', '.join(t['crates'])}{' +heif' if t['heif'] else ''}")
    return "\n".join(lines)


def write_github_output(result, path):
    flat = {
        "full": str(result["full"]).lower(),
        "rust": str(result["rust"]).lower(),
        "crates": " ".join(result["crates"]),
        "clippy_args": result["clippy_args"],
        "wasm": str(result["wasm"]).lower(),
        "parity": str(result["parity"]).lower(),
        "heif": str(result["heif"]).lower(),
        "tests": json.dumps(result["tests"], separators=(",", ":")),
    }
    with open(path, "a", encoding="utf-8") as out:
        for key, value in flat.items():
            out.write(f"{key}={value}\n")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", help="git revision to compare HEAD with (merge base)")
    ap.add_argument("--files", help="a file with one changed path per line (a path prefixed '-' counts as deleted)")
    ap.add_argument("--metadata", help="read `cargo metadata --no-deps` JSON from this file")
    ap.add_argument("--groups", type=int, default=3)
    ap.add_argument("--github-output", action="store_true", help="append the plan to $GITHUB_OUTPUT")
    args = ap.parse_args(argv)
    if bool(args.base) == bool(args.files):
        ap.error("give exactly one of --base and --files")
    pkgs = workspace_packages(load_metadata(args.metadata))
    if args.base:
        changed = changed_from_git(args.base)
    else:
        changed = []
        for line in Path(args.files).read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line:
                changed.append((line[1:], True) if line.startswith("-") else (line, False))
    result = plan(changed, pkgs, wasm_set(pkgs, ROOT / "xtask/src/layers.rs"), args.groups)
    print(summary(result, len(changed)), file=sys.stderr)
    print(json.dumps(result, indent=2))
    if args.github_output:
        write_github_output(result, os.environ["GITHUB_OUTPUT"])


if __name__ == "__main__":
    main()
