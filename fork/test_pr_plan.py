#!/usr/bin/env python3
"""Tests of fork/pr_plan.py: `python3 fork/test_pr_plan.py` (no cargo needed, no network)."""

import re
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))

import pr_plan  # noqa: E402


def pkg(dir_, deps=(), doc=True):
    return {"dir": dir_, "deps": set(deps), "doc": doc}


# a small workspace shaped like LightCraft: geom <- raster <- engine <- ui-egui <- app, testkit is a dev-dependency
PKGS = {
    "lightcraft-geom": pkg("crates/geom"),
    "lightcraft-raster": pkg("crates/raster", ["lightcraft-geom"]),
    "lightcraft-codecs": pkg("crates/codecs", ["lightcraft-raster"]),
    "lightcraft-engine": pkg("crates/engine", ["lightcraft-raster", "lightcraft-codecs", "lightcraft-testkit"]),
    "lightcraft-ui-egui": pkg("crates/ui-egui", ["lightcraft-engine", "lightcraft-testkit"]),
    "lightcraft-testkit": pkg("crates/testkit"),
    "lightcraft": pkg("apps/lightcraft", ["lightcraft-ui-egui"], doc=False),
    "lightcraft-cli": pkg("apps/lightcraft-cli", ["lightcraft-engine"], doc=False),
    "xtask": pkg("xtask"),
}
WEIGHTS = {n: 10 for n in PKGS} | {"lightcraft-ui-egui": 100, "lightcraft-engine": 60}
WASM = {"lightcraft-geom", "lightcraft-raster", "lightcraft-codecs", "lightcraft-engine", "lightcraft-ui-egui"}


def plan(*paths, gone=()):
    changed = [(p, p in gone) for p in paths]
    return pr_plan.plan(changed, PKGS, WASM, weights=WEIGHTS)


class Scope(unittest.TestCase):
    def test_leaf_crate_tests_itself_and_its_users(self):
        r = plan("crates/ui-egui/src/lib.rs")
        self.assertEqual(r["crates"], ["lightcraft", "lightcraft-ui-egui"])
        self.assertFalse(r["full"])

    def test_low_crate_tests_every_crate_above_it(self):
        r = plan("crates/raster/src/lib.rs")
        self.assertEqual(
            r["crates"],
            ["lightcraft", "lightcraft-cli", "lightcraft-codecs", "lightcraft-engine", "lightcraft-raster", "lightcraft-ui-egui"],
        )

    def test_dev_dependency_counts(self):
        self.assertIn("lightcraft-ui-egui", plan("crates/testkit/src/lib.rs")["crates"])

    def test_workspace_wide_files_test_everything(self):
        for path in ["Cargo.lock", "Cargo.toml", ".cargo/config.toml", "xtask/src/ci.rs", ".github/workflows/fork-pr.yml", "fork/known-failures.txt"]:
            r = plan(path)
            self.assertTrue(r["full"], path)
            self.assertEqual(r["crates"], sorted(PKGS), path)
            self.assertEqual(r["clippy_args"], "--workspace", path)

    def test_the_plan_script_and_the_test_runner_test_everything(self):
        for path in ["fork/pr_plan.py", "fork/skip-known-failures.sh"]:
            self.assertTrue(plan(path)["full"], path)

    def test_notes_and_fork_files_test_nothing(self):
        for path in ["docs/foo.md", "FORK.md", "fork/tests.d/x.txt", "fork/some-feature.md", "assets/ATTRIBUTION.md", "README.md"]:
            r = plan(path)
            self.assertFalse(r["rust"], path)
            self.assertEqual(r["tests"], [], path)
            self.assertFalse(r["wasm"], path)

    def test_embedded_file_counts_for_its_crate(self):
        r = plan("assets/camera-profiles/x.json")
        self.assertEqual(r["changed"], ["lightcraft-engine"])
        self.assertIn("lightcraft-ui-egui", r["crates"])

    def test_innermost_crate_owns_a_file(self):
        pkgs = dict(PKGS, **{"nested": pkg("crates/raster/nested")})
        r = pr_plan.plan([("crates/raster/nested/a.rs", False)], pkgs, WASM, weights=WEIGHTS)
        self.assertEqual(r["changed"], ["nested"])

    def test_parity_runs_for_command_crates_tracker_docs_and_deleted_files(self):
        self.assertTrue(plan("crates/engine/src/a.rs")["parity"])
        self.assertTrue(plan("docs/gaps.md")["parity"])
        self.assertTrue(plan("docs/images/a.png", gone={"docs/images/a.png"})["parity"])
        self.assertFalse(plan("crates/geom/src/a.rs")["parity"])  # the engine above it is tested, but its ids cannot change

    def test_heif_runs_when_codecs_or_a_crate_below_it_changes(self):
        self.assertTrue(plan("crates/codecs/src/a.rs")["heif"])
        self.assertTrue(plan("crates/geom/src/a.rs")["heif"])
        self.assertFalse(plan("crates/ui-egui/src/a.rs")["heif"])

    def test_wasm_runs_only_when_a_wasm_crate_is_tested(self):
        self.assertTrue(plan("crates/raster/src/a.rs")["wasm"])
        self.assertFalse(plan("apps/lightcraft/src/main.rs")["wasm"])


class Groups(unittest.TestCase):
    def test_groups_cover_every_crate_once(self):
        r = plan("Cargo.lock")
        seen = [c for t in r["tests"] for c in t["crates"]]
        self.assertEqual(sorted(seen), sorted(PKGS))
        self.assertEqual(len(r["tests"]), 3)

    def test_groups_differ_by_at_most_the_heaviest_crate(self):
        r = plan("Cargo.lock")
        weights = sorted(t["weight"] for t in r["tests"])
        self.assertLessEqual(weights[-1] - weights[0], max(WEIGHTS.values()))

    def test_the_heaviest_crate_is_alone_when_the_others_are_light(self):
        w = {"big": 100, "a": 10, "b": 10, "c": 10}
        pkgs = {n: pkg(f"crates/{n}") for n in w}
        groups = pr_plan.balance(w, 3)
        self.assertIn((100, ["big"]), groups)

    def test_a_single_crate_makes_one_group(self):
        pkgs = {"only": pkg("crates/only")}
        r = pr_plan.plan([("crates/only/a.rs", False)], pkgs, set(), weights={"only": 5})
        self.assertEqual([t["packages"] for t in r["tests"]], ["-p only"])

    def test_doc_tests_skip_crates_without_a_library(self):
        r = plan("Cargo.lock")
        docs = " ".join(t["doc_packages"] for t in r["tests"])
        self.assertIn("-p lightcraft-geom", docs)
        self.assertNotIn("-p lightcraft-cli", docs)
        self.assertNotIn("-p lightcraft ", docs + " ")

    def test_packages_are_cargo_arguments(self):
        r = plan("crates/ui-egui/src/lib.rs")
        self.assertEqual(r["clippy_args"], "-p lightcraft -p lightcraft-ui-egui")

    def test_balance_is_deterministic(self):
        w = {"a": 5, "b": 5, "c": 5, "d": 5}
        self.assertEqual(pr_plan.balance(w, 3), pr_plan.balance(dict(reversed(list(w.items()))), 3))

    def test_heif_extras_count_in_a_group(self):
        r = plan("crates/codecs/src/a.rs")
        self.assertEqual(sum(t["heif"] for t in r["tests"]), 1)


class Git(unittest.TestCase):
    def test_name_status_parsing(self):
        out = "M\0crates/a/x.rs\0D\0old.png\0R100\0from.rs\0to.rs\0A\0new.rs\0"
        self.assertEqual(
            pr_plan.parse_name_status(out),
            [("crates/a/x.rs", False), ("old.png", True), ("from.rs", True), ("to.rs", False), ("new.rs", False)],
        )

    def test_empty_diff(self):
        self.assertEqual(pr_plan.parse_name_status(""), [])


class Drift(unittest.TestCase):
    """The tables copied from the Rust xtask and ci.yml must stay equal to the originals."""

    def test_embedded_table_matches_xtask(self):
        src = (ROOT / "xtask/src/ci_quick.rs").read_text(encoding="utf-8")
        block = src.split("const EMBEDDED", 1)[1].split("];", 1)[0]
        rust = [
            (path, re.findall(r'"([^"]+)"', names))
            for path, names in re.findall(r'\("([^"]+)",\s*&\[([^\]]*)\]\)', block)
        ]
        self.assertTrue(rust)
        self.assertEqual(pr_plan.EMBEDDED, rust)

    def test_parity_and_heif_sets_match_xtask(self):
        src = (ROOT / "xtask/src/ci_quick.rs").read_text(encoding="utf-8")
        for const, mine in [("PARITY_PACKAGES", pr_plan.PARITY_PACKAGES), ("HEIF_PACKAGES", pr_plan.HEIF_PACKAGES)]:
            body = src.split(f"const {const}", 1)[1].split("];", 1)[0]
            self.assertEqual(set(re.findall(r'"([^"]+)"', body)), mine, const)

    def test_workflow_uses_the_same_fonts_commit_and_packages_as_ci_yml(self):
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        mine = (ROOT / ".github/workflows/fork-pr.yml").read_text(encoding="utf-8")
        ref = re.search(r"craft-fonts-ref:\s*([0-9a-f]{40})", ci).group(1)
        self.assertIn(ref, mine)
        block = r"packages:\s*>-\n((?: +[a-z0-9][a-z0-9 .+-]*\n)+)"
        ci_pkgs = re.search(block, ci).group(1).split()
        for found in re.findall(block, mine):  # every job installs the same packages
            self.assertEqual(sorted(found.split()), sorted(ci_pkgs))
        mine_pkgs = ci_pkgs
        self.assertEqual(sorted(ci_pkgs), sorted(mine_pkgs))

    def test_wasm_set_reads_the_layer_table(self):
        pkgs = {n: pkg(f"x/{n}") for n in ["lightcraft-geom", "lightcraft-ui-egui", "lightcraft-testkit", "lightcraft-web", "lightcraft-cli", "xtask"]}
        got = pr_plan.wasm_set(pkgs, ROOT / "xtask/src/layers.rs")
        self.assertEqual(got, {"lightcraft-geom", "lightcraft-ui-egui", "lightcraft-web"})


if __name__ == "__main__":
    unittest.main()
