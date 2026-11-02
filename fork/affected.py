#!/usr/bin/env python3
"""Which workspace crates a pull request can affect, and how Fork CI splits them (fork/ci.sh).

Reads the changed files on stdin and prints what to run. A change tests the crates with a changed file plus every
workspace crate that depends on one of them (normal, dev and build dependencies, from `cargo metadata`). A
workspace-wide file, or a file outside every crate that is not Markdown, tests every crate; a change to notes only
(Markdown outside the crates) runs the format check and nothing else. A push to `custom` passes --full: everything.

    git diff --name-only BASE HEAD | python3 -I fork/affected.py            # prints `full`, `none` or the crates
    git diff --name-status BASE HEAD | python3 -I fork/affected.py --sh --part 2/3   # shell variables for fork/ci.sh
    gh pr diff 60 --name-only | python3 -I fork/affected.py --summary       # the whole plan, for a check by hand

Input lines are a path, or `git diff --name-status` lines (`D<TAB>path`, `R100<TAB>old<TAB>new`; a deleted or
renamed-away file can break a path the parity tracker cites). The tests split into --part N/T balanced groups; group 1
also runs the one-off steps (format, lint, layers, assets, parity, wasm). Tests: `python3 fork/test_affected.py`.
Why and how: fork/github-builds.md.
"""

import argparse
import json
import os
import re
import shlex
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
    "fork/affected.py",
    "fork/test_affected.py",
    "fork/ci.sh",
}
WORKSPACE_WIDE_DIRS = (".cargo/", ".config/", ".github/", "xtask/")

# Files outside every crate folder that are not Markdown test every crate (a build script or `include_*!` may read
# them), except the ones a crate is known to compile in (EMBEDDED below) and the folders listed here, which are
# known to hold no build input. `fork/tests.d/` holds one text file of test commands per feature (FORK.md asks every
# feature to add one); only `fork/tests.sh`, run by hand on the Mac, reads them.
INERT_DIRS = ("fork/tests.d/",)

# Files outside a crate's folder that the crate compiles in. Mirrors EMBEDDED in xtask/src/ci_quick.rs, whose test
# `embedded_files_are_mapped` keeps it complete, so any other file outside every crate cannot change a build.
# test_affected.py checks that this table and the Rust one agree.
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

# Weights are seconds on a GitHub runner (measured on PR 66's runs). A crate's test time is guessed from its size
# (a test counts 1, 200 lines of Rust count 1, times SECONDS_PER_UNIT: the engine's 1830 tests took 223 s); a crate
# the guess gets wrong is measured and listed here. ui-egui's 2360 lib tests alone took 1394 s, so it sits alone in
# a group and is the floor of a run that touches it.
SECONDS_PER_UNIT = 0.1
WEIGHT_OVERRIDES = {"lightcraft-ui-egui": 1500}
# The heif extras (clippy and tests with the feature) count as a crate for the balance: 139 s warm, 350 s cold.
HEIF_WEIGHT = 250
# The one-off steps of group 1 (clippy on the planned crates, layers, assets, parity, wasm): 216 s warm, 750 s cold.
ONEOFF_WEIGHT = 400

TEST_ATTR = re.compile(r"#\[(?:\w+::)*test\b")


def run(cmd, cwd=ROOT):
    out = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, check=False)
    if out.returncode != 0:
        sys.exit(f"{' '.join(cmd)}: {out.stderr.strip()}")
    return out.stdout


def load_metadata(path=None):
    # With dependencies: the resolve section holds the features the whole workspace build turns on (see workspace_packages).
    text = Path(path).read_text(encoding="utf-8") if path else run(["cargo", "metadata", "--format-version", "1"])
    return json.loads(text)


def workspace_packages(meta, root=ROOT):
    """The workspace crates: name, folder relative to the root, workspace dependencies, doc-test support, features.

    `features` are the crate's features that `cargo test --workspace` turns on (for example `lightcraft-engine/sam`,
    which the desktop app enables). A `cargo test -p <crate>` of a subset resolves fewer features, builds a different
    crate and fails tests that need them, so every run passes these along."""
    root = Path(meta.get("workspace_root") or root)
    members = set(meta["workspace_members"])
    enabled = {n["id"]: set(n.get("features", [])) for n in (meta.get("resolve") or {}).get("nodes", [])}
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
        features = sorted(f"{p['name']}/{f}" for f in enabled.get(p["id"], ()) if f in p["features"] and f != "default")
        pkgs[p["name"]] = {"dir": rel, "deps": {d["name"] for d in p["dependencies"]}, "doc": has_doctests, "features": features}
    for p in pkgs.values():
        p["deps"] &= pkgs.keys()
    return pkgs


def parse_changed(text):
    """Input lines to [(path, gone)]: a path, or `git diff --name-status` lines (tab separated)."""
    changed = []
    for line in text.splitlines():
        if not line.strip():
            continue
        if "\t" in line:
            status, *paths = line.split("\t")
            kind = status.strip()[:1]
            if kind in "RC" and len(paths) == 2:
                changed += [(paths[0], kind == "R"), (paths[1], False)]
            elif paths:
                changed.append((paths[0], kind == "D"))
        else:
            changed.append((line.strip(), False))
    return changed


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


def is_note(path):
    return path.lower().endswith(".md")


def is_inert(path):
    return path.startswith(INERT_DIRS) if INERT_DIRS else False


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
    return max(1, round((tests + lines // 200) * SECONDS_PER_UNIT))


def balance(weights, groups, first=None):
    """Longest-first: each item goes to the lightest group. `first` (name, weight) starts group 1.
    Returns [(total, [names])] with empty groups dropped."""
    bins = [[0, []] for _ in range(groups)]
    if first:
        bins[0] = [first[1], [first[0]]]
    for name, w in sorted(weights.items(), key=lambda kv: (-kv[1], kv[0])):
        lightest = min(bins, key=lambda b: (b[0], len(b[1])))
        lightest[0] += w
        lightest[1].append(name)
    return [(total, sorted(names)) for total, names in bins if names]


def features_args(crates, pkgs, lead=""):
    """`--features a/x,b/y`: the workspace-wide features of `crates` (see workspace_packages), or ''.

    Cargo accepts `crate/feature` for a selected crate and for a direct dependency of one, so the crates' own
    dependencies in the workspace count too (a group with `ui-egui` alone still gets `lightcraft-engine/sam`)."""
    scope = set(crates).union(*(pkgs[c]["deps"] for c in crates)) if crates else set()
    names = sorted(f for c in scope for f in pkgs[c].get("features", ()))
    return f"{lead}--features {','.join(names)}" if names else ""


def plan(changed, pkgs, wasm, groups=3, weights=None, force_full=False):
    """What the checks run. `changed` is [(path, gone)]; `weights` maps crate to test weight (default: measured on disk)."""
    direct, full, tracker, gone_any = set(), force_full, False, False
    for path, gone in changed:
        o = owner(path, pkgs)
        embedded = [n for n in embedders(path) if n in pkgs]
        if is_workspace_wide(path) or (not o and not embedded and not is_note(path) and not is_inert(path)):
            full = True
        tracker |= path in PARITY_DOCS
        gone_any |= gone
        if o:
            direct.add(o)
        direct.update(embedded)
    planned = set(pkgs) if full else reverse_closure(direct, pkgs)
    rust = bool(planned)
    # parity and heif only matter when something compiled changed; a notes-only change runs the format check alone
    parity = rust and (full or tracker or gone_any or bool(direct & PARITY_PACKAGES))
    heif = full or bool(planned & HEIF_PACKAGES)
    weights = weights or {}
    items = {n: weights[n] if n in weights else weight(n, pkgs) for n in planned}
    if heif:
        items["+heif"] = HEIF_WEIGHT
    first = ("+oneoff", ONEOFF_WEIGHT if rust else 1)
    bins = balance(items, groups, first)
    tests = []
    for number, (total, names) in enumerate(bins, 1):
        crates = [n for n in names if not n.startswith("+")]
        doc = [n for n in crates if pkgs[n]["doc"]]
        tests.append(
            {
                "group": number,
                "name": f"{number}/{len(bins)}",
                "packages": " ".join(f"-p {n}" for n in crates),
                "doc_packages": " ".join(f"-p {n}" for n in doc),
                "features": features_args(crates, pkgs),
                "doc_features": features_args(doc, pkgs),
                "heif": "+heif" in names,
                "oneoff": "+oneoff" in names,
                "weight": total,
                "crates": crates,
            }
        )
    return {
        "full": full,
        "rust": rust,
        "changed": sorted(direct),
        "crates": sorted(planned),
        "clippy_args": "--workspace" if full else " ".join(f"-p {n}" for n in sorted(planned)) + features_args(planned, pkgs, " "),
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
        extras = (" +one-off steps" if t["oneoff"] else "") + (" +heif" if t["heif"] else "")
        lines.append(f"test group {t['name']} (weight {t['weight']}): {', '.join(t['crates'])}{extras}")
    return "\n".join(lines)


def shell_vars(result, part):
    """Shell assignments for fork/ci.sh: what test group `part` (1-based) runs."""
    t = next((t for t in result["tests"] if t["group"] == part), None)
    scope = "full" if result["full"] else " ".join(result["crates"]) if result["rust"] else "none"
    flag = lambda v: "1" if v else "0"  # noqa: E731
    mine = t or {}
    out = {
        "SCOPE": scope,
        "RUST": flag(result["rust"]),
        "ONEOFF": flag(mine.get("oneoff")),
        "CLIPPY_ARGS": result["clippy_args"],
        "HEIF": flag(result["heif"]),
        "PARITY": flag(result["parity"]),
        "WASM": flag(result["wasm"]),
        "PACKAGES": mine.get("packages", ""),
        "DOC_PACKAGES": mine.get("doc_packages", ""),
        "FEATURES": mine.get("features", ""),
        "DOC_FEATURES": mine.get("doc_features", ""),
        "HEIF_TESTS": flag(mine.get("heif")),
    }
    return "\n".join(f"{k}={shlex.quote(v)}" for k, v in out.items())


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--full", action="store_true", help="test everything (a push to custom, or FORK_CI_FULL=1)")
    ap.add_argument("--part", default="1/3", help="N/T: the test group to print variables for (default 1/3)")
    ap.add_argument("--metadata", help="read `cargo metadata` JSON from this file instead of running cargo")
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--sh", action="store_true", help="print shell variables for the group (fork/ci.sh)")
    mode.add_argument("--summary", action="store_true", help="print the whole plan, readable")
    mode.add_argument("--json", action="store_true", help="print the whole plan as JSON")
    args = ap.parse_args(argv)
    part, _, total = args.part.partition("/")
    part, total = int(part), int(total or 3)
    pkgs = workspace_packages(load_metadata(args.metadata))
    changed = parse_changed(sys.stdin.read())
    result = plan(changed, pkgs, wasm_set(pkgs, ROOT / "xtask/src/layers.rs"), total, force_full=args.full)
    if args.sh:
        print(shell_vars(result, part))
    elif args.summary:
        print(summary(result, len(changed)))
    elif args.json:
        print(json.dumps(result, indent=2))
    else:
        print("full" if result["full"] else " ".join(result["crates"]) if result["rust"] else "none")


if __name__ == "__main__":
    main()
