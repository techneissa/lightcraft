#!/usr/bin/env python3
"""Tests of fork/affected.py: `python3 fork/test_pr_plan.py` (no cargo needed, no network)."""

import re
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))

import affected as pr_plan  # noqa: E402


def pkg(dir_, deps=(), doc=True, features=()):
    return {"dir": dir_, "deps": set(deps), "doc": doc, "features": list(features)}


# a small workspace shaped like LightCraft: geom <- raster <- engine <- ui-egui <- app, testkit is a dev-dependency
PKGS = {
    "lightcraft-geom": pkg("crates/geom"),
    "lightcraft-raster": pkg("crates/raster", ["lightcraft-geom"]),
    "lightcraft-codecs": pkg("crates/codecs", ["lightcraft-raster"]),
    "lightcraft-engine": pkg(
        "crates/engine", ["lightcraft-raster", "lightcraft-codecs", "lightcraft-testkit"], features=["lightcraft-engine/sam"]
    ),
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
        for path in ["Cargo.lock", "Cargo.toml", ".cargo/config.toml", "xtask/src/ci.rs", ".github/workflows/fork-ci.yml", "fork/known-failures.txt"]:
            r = plan(path)
            self.assertTrue(r["full"], path)
            self.assertEqual(r["crates"], sorted(PKGS), path)
            self.assertEqual(r["clippy_args"], "--workspace", path)

    def test_the_ci_files_and_the_test_runner_test_everything(self):
        for path in ["fork/affected.py", "fork/skip-known-failures.sh"]:
            self.assertTrue(plan(path)["full"], path)

    def test_notes_run_the_format_check_only(self):
        for path in ["docs/foo.md", "FORK.md", "fork/some-feature.md", "assets/ATTRIBUTION.md", "README.md", "docs/gaps.md"]:
            r = plan(path)
            self.assertFalse(r["rust"], path)
            self.assertFalse(r["full"], path)
            self.assertFalse(r["wasm"] or r["parity"] or r["heif"], path)
            self.assertEqual(len(r["tests"]), 1, path)
            self.assertTrue(r["tests"][0]["oneoff"], path)
            self.assertEqual(r["tests"][0]["packages"], "", path)

    def test_a_file_outside_every_crate_that_is_not_markdown_tests_everything(self):
        for path in ["fork/tests.sh", "assets/logo.png", "LICENSE-MIT", "packaging/linux/a.sh", "fork/sync-docs.py"]:
            self.assertTrue(plan(path)["full"], path)

    def test_the_fork_test_lists_are_no_build_input(self):
        r = plan("fork/tests.d/x.txt", "fork/x.md")
        self.assertFalse(r["full"])
        self.assertFalse(r["rust"])

    def test_markdown_inside_a_crate_counts_for_the_crate(self):
        self.assertEqual(plan("crates/geom/README.md")["changed"], ["lightcraft-geom"])

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
        self.assertTrue(plan("docs/gaps.md", "crates/geom/src/a.rs")["parity"])
        self.assertTrue(plan("crates/geom/a.png", gone={"crates/geom/a.png"})["parity"])
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
        seen = [c for t in r["tests"] for c in t["crates"] if c not in pr_plan.SHARDS]
        self.assertEqual(sorted(seen), sorted(set(PKGS) - set(pr_plan.SHARDS)))
        self.assertEqual(len(r["tests"]), pr_plan.GROUPS)
        self.assertEqual([t["group"] for t in r["tests"] if t["oneoff"]], [1])  # the one-off steps run in group 1 only

    def test_groups_differ_by_at_most_the_heaviest_crate(self):
        r = plan("Cargo.lock")
        weights = sorted(t["weight"] for t in r["tests"])
        self.assertLessEqual(weights[-1] - weights[0], max(max(WEIGHTS.values()), pr_plan.ONEOFF_WEIGHT))

    def test_the_heaviest_crate_is_alone_when_the_others_are_light(self):
        w = {"big": 100, "a": 10, "b": 10, "c": 10}
        groups = pr_plan.balance(w, 3)
        self.assertIn((100, ["big"]), groups)

    def test_the_seed_starts_group_one(self):
        groups = pr_plan.balance({"a": 10, "b": 10}, 3, ("+oneoff", 50))
        self.assertEqual(groups[0], (50, ["+oneoff"]))

    def test_a_single_crate_makes_one_group(self):
        pkgs = {"only": pkg("crates/only")}
        r = pr_plan.plan([("crates/only/a.rs", False)], pkgs, set(), weights={"only": 5})
        self.assertEqual([t["packages"] for t in r["tests"]], ["", "-p only"])  # group 1 holds the one-off steps

    def test_doc_tests_skip_crates_without_a_library(self):
        r = plan("Cargo.lock")
        docs = " ".join(t["doc_packages"] for t in r["tests"])
        self.assertIn("-p lightcraft-geom", docs)
        self.assertNotIn("-p lightcraft-cli", docs)
        self.assertNotIn("-p lightcraft ", docs + " ")

    def test_packages_are_cargo_arguments(self):
        r = plan("crates/ui-egui/src/lib.rs")
        self.assertEqual(r["clippy_args"], "-p lightcraft -p lightcraft-ui-egui --features lightcraft-engine/sam")

    def test_balance_is_deterministic(self):
        w = {"a": 5, "b": 5, "c": 5, "d": 5}
        self.assertEqual(pr_plan.balance(w, 3), pr_plan.balance(dict(reversed(list(w.items()))), 3))

    def test_heif_extras_count_in_a_group(self):
        r = plan("crates/codecs/src/a.rs")
        self.assertEqual(sum(t["heif"] for t in r["tests"]), 1)


class Shards(unittest.TestCase):
    def shards(self, r, crate="lightcraft-ui-egui"):
        """{group: [shard indices]} and the shard count, read back from each group's `shard` value."""
        out, counts = {}, set()
        for t in r["tests"]:
            for spec in t["shard"].split():
                name, _, which = spec.partition("=")
                if name == crate:
                    ix, _, k = which.partition("/")
                    out[t["group"]] = [int(i) for i in ix.split(",")]
                    counts.add(int(k))
        return out, counts

    def test_a_sharded_crate_runs_every_shard_exactly_once(self):
        r = plan("Cargo.lock")
        groups, counts = self.shards(r)
        k = min(pr_plan.SHARDS["lightcraft-ui-egui"], pr_plan.GROUPS)
        self.assertEqual(counts, {k})
        self.assertEqual(sorted(i for ix in groups.values() for i in ix), list(range(1, k + 1)))
        for t in r["tests"]:  # a group with a shard tests the crate; one without does not
            self.assertEqual("lightcraft-ui-egui" in t["crates"], t["group"] in groups)

    def test_shards_never_outnumber_the_groups(self):
        r = pr_plan.plan([("Cargo.lock", False)], PKGS, WASM, groups=2, weights=WEIGHTS)
        _, counts = self.shards(r)
        self.assertEqual(counts, {2})

    def test_one_group_runs_the_crate_unsplit(self):
        r = pr_plan.plan([("Cargo.lock", False)], PKGS, WASM, groups=1, weights=WEIGHTS)
        self.assertEqual([t["shard"] for t in r["tests"]], [""])
        self.assertIn("lightcraft-ui-egui", r["tests"][0]["crates"])

    def test_a_sharded_crate_runs_its_doc_tests_once_with_shard_one(self):
        r = plan("Cargo.lock")
        groups, _ = self.shards(r)
        with_docs = [t["group"] for t in r["tests"] if "lightcraft-ui-egui" in t["doc_packages"].split()]
        self.assertEqual(with_docs, [g for g, ix in groups.items() if 1 in ix])

    def test_the_shell_gets_the_shard(self):
        r = plan("Cargo.lock")
        groups, _ = self.shards(r)
        group, ix = next(iter(groups.items()))
        v = pr_plan.shell_vars(r, group)
        self.assertIn(f"lightcraft-ui-egui={','.join(map(str, ix))}/", v)


class Features(unittest.TestCase):
    """A subset build must turn on the features the whole workspace build turns on (the desktop app enables engine/sam)."""

    def test_a_group_gets_the_features_of_its_crates_and_their_direct_dependencies(self):
        r = plan("Cargo.lock")
        by = {t["crates"][0]: t for t in r["tests"] if t["crates"]}
        engine_leg = next(t for t in r["tests"] if "lightcraft-engine" in t["crates"])
        ui_leg = next(t for t in r["tests"] if "lightcraft-ui-egui" in t["crates"])
        self.assertEqual(engine_leg["features"], "--features lightcraft-engine/sam")
        self.assertEqual(ui_leg["features"], "--features lightcraft-engine/sam")  # engine is ui-egui's direct dependency
        self.assertTrue(by)

    def test_no_features_no_flag(self):
        self.assertEqual(pr_plan.features_args(["lightcraft-geom"], PKGS), "")

    def test_clippy_gets_them_too_unless_it_runs_the_whole_workspace(self):
        self.assertTrue(plan("crates/ui-egui/src/a.rs")["clippy_args"].endswith("--features lightcraft-engine/sam"))
        self.assertEqual(plan("Cargo.lock")["clippy_args"], "--workspace")

    def test_features_come_from_the_resolve_section_of_cargo_metadata(self):
        meta = {
            "workspace_root": "/w",
            "workspace_members": ["a 0.1.0", "b 0.1.0"],
            "resolve": {"nodes": [{"id": "a 0.1.0", "features": ["default", "sam", "dep:x", "unknown"]}, {"id": "b 0.1.0", "features": []}]},
            "packages": [
                {"id": "a 0.1.0", "name": "a", "manifest_path": "/w/crates/a/Cargo.toml", "dependencies": [], "targets": [], "features": {"default": [], "sam": []}},
                {"id": "b 0.1.0", "name": "b", "manifest_path": "/w/crates/b/Cargo.toml", "dependencies": [{"name": "a"}], "targets": [], "features": {}},
            ],
        }
        pkgs = pr_plan.workspace_packages(meta)
        self.assertEqual(pkgs["a"]["features"], ["a/sam"])  # declared and enabled; not default, not unknown
        self.assertEqual(pkgs["b"]["features"], [])
        self.assertEqual(pkgs["b"]["deps"], {"a"})


class Input(unittest.TestCase):
    def test_plain_paths(self):
        self.assertEqual(pr_plan.parse_changed("a/b.rs\n\n c.md \n"), [("a/b.rs", False), ("c.md", False)])

    def test_name_status_lines(self):
        out = "M\tcrates/a/x.rs\nD\told.png\nR100\tfrom.rs\tto.rs\nA\tnew.rs\n"
        self.assertEqual(
            pr_plan.parse_changed(out),
            [("crates/a/x.rs", False), ("old.png", True), ("from.rs", True), ("to.rs", False), ("new.rs", False)],
        )

    def test_empty_diff(self):
        self.assertEqual(pr_plan.parse_changed(""), [])

    def test_a_push_tests_everything(self):
        r = pr_plan.plan([], PKGS, WASM, weights=WEIGHTS, force_full=True)
        self.assertTrue(r["full"])
        self.assertEqual(r["crates"], sorted(PKGS))
        self.assertTrue(r["parity"] and r["heif"])


class ShellVars(unittest.TestCase):
    def vars(self, part, *paths):
        out = pr_plan.shell_vars(plan(*paths), part)
        return dict(line.split("=", 1) for line in out.splitlines())

    def test_group_one_runs_the_one_off_steps_and_the_others_do_not(self):
        self.assertEqual(self.vars(1, "Cargo.lock")["ONEOFF"], "1")
        self.assertEqual(self.vars(2, "Cargo.lock")["ONEOFF"], "0")
        self.assertEqual(self.vars(pr_plan.GROUPS, "Cargo.lock")["ONEOFF"], "0")

    def test_values_are_shell_quoted(self):
        v = pr_plan.shell_vars(plan("crates/ui-egui/src/a.rs"), 1)
        self.assertIn("CLIPPY_ARGS='-p lightcraft", v)

    def test_a_group_that_does_not_exist_runs_nothing(self):
        v = self.vars(pr_plan.GROUPS, "docs/foo.md")
        self.assertEqual((v["ONEOFF"], v["PACKAGES"], v["RUST"], v["SCOPE"]), ("0", "''", "0", "none"))

    def test_notes_only_scope_is_none_and_full_is_full(self):
        self.assertEqual(self.vars(1, "docs/foo.md")["SCOPE"], "none")
        self.assertEqual(self.vars(1, "Cargo.lock")["SCOPE"], "full")

    def test_doc_features_cover_only_the_crates_with_doc_tests(self):
        r = plan("Cargo.lock")
        leg = next(t for t in r["tests"] if "lightcraft-engine" in t["crates"])
        self.assertEqual(leg["doc_features"], "--features lightcraft-engine/sam")
        leg = next(t for t in r["tests"] if "lightcraft" in t["crates"])  # the app has no library: no doc tests
        self.assertNotIn("-p lightcraft ", leg["doc_packages"] + " ")


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

    def test_xtask_ci_steps_are_the_ones_fork_ci_sh_runs(self):
        """A step upstream adds to `cargo xtask ci` has to be added to fork/ci.sh (and here) by hand."""
        src = (ROOT / "xtask/src/ci.rs").read_text(encoding="utf-8")
        body = src.split("let mut steps: Vec<Step> = vec![", 1)[1].split("\n    ];", 1)[0]
        names = set(re.findall(r'^\s{12}"([a-z-]+)",$', body, re.M))
        names |= set(re.findall(r'steps\.push\(\(\s*"([a-z-]+)"', src))
        known = {"fmt", "clippy", "heif", "test", "gpu-backends", "parity", "layers", "assets", "wasm"}
        self.assertEqual(names, known)
        script = (ROOT / "fork/ci.sh").read_text(encoding="utf-8")
        for step in known - {"gpu-backends"}:  # no GPU on the runners
            self.assertIn(step, script)

    def test_the_workflow_runs_every_group(self):
        wf = (ROOT / ".github/workflows/fork-ci.yml").read_text(encoding="utf-8")
        parts = re.search(r"part: \[([0-9, ]+)\]", wf)
        self.assertIsNotNone(parts)
        self.assertEqual([int(x) for x in parts.group(1).split(",")], list(range(1, pr_plan.GROUPS + 1)))
        self.assertIn("--part ${{ matrix.part }}/%d" % pr_plan.GROUPS, wf)

    def test_wasm_set_reads_the_layer_table(self):
        pkgs = {n: pkg(f"x/{n}") for n in ["lightcraft-geom", "lightcraft-ui-egui", "lightcraft-testkit", "lightcraft-web", "lightcraft-cli", "xtask"]}
        got = pr_plan.wasm_set(pkgs, ROOT / "xtask/src/layers.rs")
        self.assertEqual(got, {"lightcraft-geom", "lightcraft-ui-egui", "lightcraft-web"})


if __name__ == "__main__":
    unittest.main()
