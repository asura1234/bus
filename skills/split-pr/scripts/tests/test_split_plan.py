"""Contract tests for split_plan.py.

Each scenario builds a throwaway repository whose source branch holds four
commits: a and b are independent, c edits a's file (stacks on a), and d edits
both files (stacks on a and b). `origin/master` is a plain ref so landing a
part can be simulated with a squash commit.
"""

import copy
import io
import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from split_plan import main  # noqa: E402
from split_plan_model import PlanError, parse_plan, shape  # noqa: E402


GIT_ENV = {
    "GIT_AUTHOR_NAME": "t",
    "GIT_AUTHOR_EMAIL": "t@t",
    "GIT_COMMITTER_NAME": "t",
    "GIT_COMMITTER_EMAIL": "t@t",
    "GIT_CONFIG_GLOBAL": os.devnull,
    "GIT_CONFIG_NOSYSTEM": "1",
}


class Repo:
    def __init__(self, root: Path) -> None:
        self.root = root
        root.mkdir()
        self.git("init", "-q", "-b", "master")
        self.write("core.txt", "core\n")
        self.commit("chore: base")
        self.base = self.git("rev-parse", "HEAD")
        self.git("update-ref", "refs/remotes/origin/master", self.base)
        self.git("switch", "-q", "-c", "feat/source")
        self.write("a.txt", "a1\na2\na3\na4\na5\n")
        self.c1 = self.commit("feat: add a")
        self.write("b.txt", "b1\nb2\nb3\nb4\nb5\n")
        self.c2 = self.commit("feat: add b")
        self.write("a.txt", "a1\nA2\na3\na4\na5\n")
        self.c3 = self.commit("feat: extend a")
        self.write("a.txt", "a1\nA2\na3\na4\nA5\n")
        self.write("b.txt", "b1\nb2\nb3\nb4\nB5\n")
        self.c4 = self.commit("feat: join a and b")
        self.source = self.git("rev-parse", "HEAD")
        self.git("switch", "-q", "master")

    def git(self, *args: str) -> str:
        result = subprocess.run(
            ["git", "-C", str(self.root), *args],
            capture_output=True,
            text=True,
            env={**os.environ, **GIT_ENV},
        )
        if result.returncode != 0:
            raise AssertionError(f"git {args}: {result.stderr}")
        return result.stdout.strip()

    def write(self, name: str, text: str) -> None:
        (self.root / name).write_text(text)

    def commit(self, message: str) -> str:
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD")

    def build(self, branch: str, start: str, *commits: str) -> None:
        self.git("switch", "-q", "-c", branch, start)
        for sha in commits:
            self.git("cherry-pick", sha)
        self.git("switch", "-q", "master")

    def run(self, *args: str) -> str:
        """Run split_plan main inside the repo; return stdout, raise on failure."""
        code, out, err = self.run_status(*args)
        if code != 0:
            raise AssertionError(f"split_plan {args} exited {code}: {err}{out}")
        return out

    def run_status(self, *args: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        previous = {key: os.environ.get(key) for key in GIT_ENV}
        os.environ.update(GIT_ENV)
        try:
            with redirect_stdout(out), redirect_stderr(err):
                code = main(["--repo", str(self.root), *args])
        finally:
            for key, value in previous.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
        return code, out.getvalue(), err.getvalue()


def part(pid: str, commits: list[str], deps: list[str] = ()) -> dict:
    return {
        "id": pid,
        "branch": f"split/{pid}",
        "title": f"feat: part {pid}",
        "depends_on": list(deps),
        "commits": commits,
        "onto": None,
        "tip": None,
        "pr": None,
        "landed": False,
    }


def plan_data(repo: Repo, parts: list[dict], *, policy: str = "wait", left: list[str] = ()) -> dict:
    return {
        "schema": "split-pr-plan/1",
        "source": {"branch": "feat/source", "sha": repo.source},
        "base": {"ref": "origin/master", "sha": repo.base},
        "multi_parent": policy,
        "parts": parts,
        "left_on_source": [{"sha": sha, "reason": "serves only the source branch"} for sha in left],
    }


class ScenarioTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Repo(Path(self.tmp.name) / "repo")
        self.plan_path = Path(self.tmp.name) / "plan.json"

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def write_plan(self, data: dict) -> str:
        self.plan_path.write_text(json.dumps(data))
        return str(self.plan_path)

    def mixed(self, policy: str = "wait") -> dict:
        r = self.repo
        return plan_data(
            r,
            [part("a", [r.c1]), part("b", [r.c2]), part("c", [r.c3], ["a"]), part("d", [r.c4], ["a", "b"])],
            policy=policy,
        )

    def train(self) -> dict:
        r = self.repo
        return plan_data(r, [part("a", [r.c1]), part("c", [r.c3], ["a"])], left=[r.c2, r.c4])

    def stored(self) -> dict:
        return json.loads(self.plan_path.read_text())


class ValidationTest(ScenarioTest):
    def test_shapes(self) -> None:
        r = self.repo
        parallel = plan_data(r, [part("a", [r.c1, r.c3]), part("b", [r.c2, r.c4])])
        self.assertEqual(shape(parse_plan(parallel)), "parallel")
        self.assertEqual(shape(parse_plan(self.train())), "train")
        self.assertEqual(shape(parse_plan(self.mixed())), "mixed")

    def test_check_reports_shape_and_dependency_order(self) -> None:
        data = self.mixed()
        data["parts"].reverse()  # d, c, b, a: order must still put parents first
        out = json.loads(self.repo.run("check", self.write_plan(data)))
        self.assertEqual(out["shape"], "mixed")
        self.assertEqual(out["order"], ["b", "a", "d", "c"])

    def test_malformed_plans_fail_closed(self) -> None:
        r = self.repo
        cases = {
            "unknown key": lambda d: d.update(extra=1),
            "unknown part key": lambda d: d["parts"][0].update(paths=[]),
            "cycle": lambda d: d["parts"][0]["depends_on"].append("c"),
            "unknown dep": lambda d: d["parts"][1]["depends_on"].append("zz"),
            "self dep": lambda d: d["parts"][0]["depends_on"].append("a"),
            "duplicate branch": lambda d: d["parts"][1].update(branch="split/a"),
            "source branch reused": lambda d: d["parts"][1].update(branch="feat/source"),
            "implicit base": lambda d: d["base"].update(ref="master"),
            "bad policy": lambda d: d.update(multi_parent="auto"),
            "landed without pr": lambda d: d["parts"][0].update(landed=True),
            "landed before parent": lambda d: d["parts"][2].update(landed=True, pr=3),
            "short sha": lambda d: d["parts"][0].update(commits=[r.c1[:9]]),
            "tip without onto": lambda d: d["parts"][0].update(tip=r.c1),
            "single part": lambda d: d.update(parts=d["parts"][:1]),
            "duplicate pr": lambda d: [d["parts"][i].update(pr=7) for i in (0, 1)],
            "left as bare sha": lambda d: d.update(left_on_source=[r.c4]),
            "left without reason": lambda d: d.update(left_on_source=[{"sha": r.c4, "reason": " "}]),
        }
        for name, mutate in cases.items():
            with self.subTest(name):
                data = self.mixed()
                mutate(data)
                with self.assertRaises(PlanError):
                    parse_plan(data)

    def test_git_coverage_fails_closed(self) -> None:
        r = self.repo
        cases = {
            "commit unassigned": lambda d: d["parts"].pop(),
            "commit outside range": lambda d: d["parts"][0]["commits"].append(r.base),
            "assigned and left": lambda d: d["left_on_source"].append({"sha": r.c1, "reason": "x"}),
            "out of source order": lambda d: d["parts"][0].update(commits=[r.c3, r.c1]),
            "invalid branch": lambda d: d["parts"][0].update(branch="bad..name"),
        }
        for name, mutate in cases.items():
            with self.subTest(name):
                data = self.mixed()
                if name == "out of source order":
                    data["parts"][2]["commits"] = [r.c4]
                    data["parts"].pop()
                mutate(data)
                code, _out, err = r.run_status("check", self.write_plan(data))
                self.assertEqual(code, 2, err)

    def test_render_shows_table_and_graph(self) -> None:
        out = self.repo.run("render", self.write_plan(self.mixed()))
        self.assertIn("Shape: mixed", out)
        self.assertIn("| 4 | d: feat: part d | `split/d` | `merge(split/a, split/b)` | a, b |", out)
        self.assertIn("| Files | Lines |", out)
        self.assertRegex(out, r"\| 1 \| a: .* \| 1 \| \+5 / -0 \|")
        self.assertRegex(out, r"\| 4 \| d: .* \| 2 \| \+2 / -2 \|")
        self.assertIn("- d: a.txt, b.txt", out)
        self.assertIn("Source total: ", out)
        self.assertIn("Files per part:", out)
        self.assertIn("    d --> b", out)
        self.assertIn("    a --> base", out)

    def test_render_lists_left_commits_with_their_reason(self) -> None:
        r = self.repo
        out = r.run("render", self.write_plan(self.train()))
        self.assertIn(f"- {r.c2[:9]}: serves only the source branch", out)

    def test_render_counts_the_effective_diff_not_commit_churn(self) -> None:
        r = self.repo
        data = plan_data(r, [part("a", [r.c1, r.c3]), part("b", [r.c2])], left=[r.c4])
        out = r.run("render", self.write_plan(data))
        # c1 adds five lines and c3 rewrites one of them: the PR shows +5 / -0.
        self.assertRegex(out, r"\| 1 \| a: .* \| 1 \| \+5 / -0 \|")
        self.assertNotIn("~", out.split("Source total")[0])


class ProbeTest(ScenarioTest):
    def probe(self, data: dict) -> tuple[int, dict]:
        code, out, err = self.repo.run_status("probe", self.write_plan(data))
        self.assertIn(code, (0, 1), err)
        return code, {p["part"]: p for p in json.loads(out)["parts"]}

    def test_declared_dependencies_are_proven_by_replay(self) -> None:
        code, parts = self.probe(self.mixed())
        self.assertEqual(code, 0)
        self.assertEqual(parts["a"]["status"], "independent")
        self.assertEqual(parts["c"]["status"], "textual-dependency")
        self.assertEqual(parts["c"]["alone"], "conflict")
        self.assertEqual(parts["d"]["status"], "textual-dependency")

    def test_undeclared_dependency_fails_and_names_the_overlap(self) -> None:
        data = self.mixed()
        data["parts"][2]["depends_on"] = []
        code, parts = self.probe(data)
        self.assertEqual(code, 1)
        self.assertEqual(parts["c"]["status"], "missing-dependency")
        self.assertEqual(parts["c"]["overlaps"], ["a"])

    def test_dependency_without_textual_need_requires_a_build_check(self) -> None:
        r = self.repo
        data = plan_data(r, [part("a", [r.c1, r.c3]), part("b", [r.c2], ["a"])], left=[r.c4])
        _code, parts = self.probe(data)
        self.assertEqual(parts["b"]["status"], "build-check-dependency")

    def test_probe_never_touches_refs_or_worktree(self) -> None:
        before = self.repo.git("for-each-ref")
        self.probe(self.mixed())
        self.assertEqual(self.repo.git("for-each-ref"), before)
        self.assertEqual(self.repo.git("status", "--porcelain"), "")


class StackTest(ScenarioTest):
    def build_train(self) -> str:
        r = self.repo
        path = self.write_plan(self.train())
        r.build("split/a", r.base, r.c1)
        r.run("record", path, "--part", "a")
        r.build("split/c", "split/a", r.c3)
        r.run("record", path, "--part", "c")
        return path

    def test_record_rejects_a_branch_on_the_wrong_base(self) -> None:
        r = self.repo
        path = self.write_plan(self.train())
        r.build("split/a", r.base, r.c1)
        r.git("switch", "-q", "split/a")
        r.write("core.txt", "core fixed\n")
        r.commit("fix: review fix on a")  # split/a tip is now unique
        r.git("switch", "-q", "master")
        r.build("split/c", r.base, r.c1, r.c3)  # flattened instead of stacked
        code, _out, err = r.run_status("record", path, "--part", "c")
        self.assertEqual(code, 2)
        self.assertIn("does not sit on split/a", err)

    def test_restack_after_review_fix_on_parent(self) -> None:
        r = self.repo
        path = self.build_train()
        self.assertEqual(json.loads(r.run("restack", path))["status"], "current")
        r.git("switch", "-q", "split/a")
        r.write("core.txt", "core fixed\n")
        r.commit("fix: review fix on a")
        r.git("switch", "-q", "master")
        steps = json.loads(r.run("restack", path))["steps"]
        self.assertEqual([s["part"] for s in steps], ["c"])
        old_onto = self.stored()["parts"][1]["onto"]
        self.assertEqual(steps[0]["commands"], [f"git rebase --onto split/a {old_onto} split/c"])
        self.assertNotIn("push", steps[0])  # no PR yet
        r.git(*steps[0]["commands"][0].split()[1:])
        r.git("switch", "-q", "master")
        r.run("record", path, "--part", "c", "--pr", "12")
        self.assertEqual(json.loads(r.run("restack", path))["status"], "current")

    def test_restack_after_parent_squash_lands(self) -> None:
        r = self.repo
        path = self.build_train()
        r.run("record", path, "--part", "a", "--pr", "11")
        r.run("record", path, "--part", "c", "--pr", "12")
        r.git("switch", "-q", "--detach", r.base)
        r.git("merge", "-q", "--squash", "split/a")
        r.git("commit", "-q", "-m", "feat: part a (#11)")
        r.git("update-ref", "refs/remotes/origin/master", r.git("rev-parse", "HEAD"))
        r.git("switch", "-q", "master")
        r.run("record", path, "--part", "a", "--landed")
        steps = json.loads(r.run("restack", path))["steps"]
        self.assertEqual(len(steps), 1)
        step = steps[0]
        self.assertEqual(step["onto_ref"], "origin/master")
        self.assertEqual(step["retarget"], "gh pr edit 12 --base master")
        self.assertIn("--force-with-lease=refs/heads/split/c:", step["push"])
        r.git(*step["commands"][0].split()[1:])
        r.git("switch", "-q", "master")
        r.run("record", path, "--part", "c")
        self.assertEqual(json.loads(r.run("restack", path))["status"], "current")
        # The restacked child carries only its own commit on the squashed base.
        self.assertEqual(r.git("rev-list", "--count", "origin/master..split/c"), "1")

    def test_multi_parent_part_sits_on_a_merge_of_its_parents(self) -> None:
        r = self.repo
        path = self.write_plan(self.mixed())
        r.build("split/a", r.base, r.c1)
        r.build("split/b", r.base, r.c2)
        r.build("split/c", "split/a", r.c3)
        for pid in ("a", "b", "c"):
            r.run("record", path, "--part", pid)
        r.git("switch", "-q", "--detach", "split/a")
        r.git("merge", "-q", "--no-ff", "-m", "chore: integrate a, b for d", "split/b")
        merge = r.git("rev-parse", "HEAD")
        r.git("switch", "-q", "master")
        r.build("split/d", merge, r.c4)
        code, _out, err = r.run_status("record", path, "--part", "d")
        self.assertEqual(code, 2)
        self.assertIn("--onto", err)
        r.run("record", path, "--part", "d", "--onto", merge)
        self.assertEqual(json.loads(r.run("coverage", path))["status"], "ok")
        # A fix on b makes d stale but leaves the train a -> c alone.
        r.git("switch", "-q", "split/b")
        r.write("b.txt", "b0\nb2\nb3\nb4\nb5\n")
        r.commit("fix: review fix on b")
        r.git("switch", "-q", "master")
        steps = json.loads(r.run("restack", path))["steps"]
        self.assertEqual([s["part"] for s in steps], ["d"])
        self.assertEqual(steps[0]["commands"][0], "git switch --detach refs/heads/split/a")
        self.assertTrue(steps[0]["commands"][-1].startswith("git rebase --onto HEAD "))

    def test_merge_policy_restack_republishes_the_integration_base(self) -> None:
        r = self.repo
        data = self.mixed(policy="merge")
        path = self.write_plan(data)
        r.build("split/a", r.base, r.c1)
        r.build("split/b", r.base, r.c2)
        r.build("split/c", "split/a", r.c3)
        r.git("switch", "-q", "--detach", "split/a")
        r.git("merge", "-q", "--no-ff", "-m", "chore: integrate a, b for d", "split/b")
        merge = r.git("rev-parse", "HEAD")
        r.git("switch", "-q", "master")
        r.build("split/d", merge, r.c4)
        for pid, number in (("a", 11), ("b", 12), ("c", 13)):
            r.run("record", path, "--part", pid, "--pr", str(number))
        r.run("record", path, "--part", "d", "--onto", merge, "--pr", "14")
        r.git("switch", "-q", "split/a")
        r.write("core.txt", "core fixed\n")
        r.commit("fix: review fix on a")
        r.git("switch", "-q", "master")
        steps = {s["part"]: s for s in json.loads(r.run("restack", path))["steps"]}
        self.assertEqual(sorted(steps), ["c", "d"])
        self.assertIn("git branch -f split/d--base HEAD", steps["d"]["commands"])
        self.assertEqual(
            steps["d"]["push_base"],
            f"git push --force-with-lease=refs/heads/split/d--base:{merge} "
            "origin refs/heads/split/d--base:refs/heads/split/d--base",
        )
        self.assertNotIn("retarget", steps["c"])

    def test_restack_partial_fan_in_keeps_the_landed_parent(self) -> None:
        r = self.repo
        path = self.write_plan(self.mixed(policy="merge"))
        r.build("split/a", r.base, r.c1)
        r.build("split/b", r.base, r.c2)
        r.build("split/c", "split/a", r.c3)
        r.git("switch", "-q", "--detach", "split/a")
        r.git("merge", "-q", "--no-ff", "-m", "chore: integrate a, b for d", "split/b")
        merge = r.git("rev-parse", "HEAD")
        r.git("switch", "-q", "master")
        r.build("split/d", merge, r.c4)
        for pid, number in (("a", 11), ("b", 12), ("c", 13)):
            r.run("record", path, "--part", pid, "--pr", str(number))
        r.run("record", path, "--part", "d", "--onto", merge, "--pr", "14")
        r.git("switch", "-q", "--detach", r.base)
        r.git("merge", "-q", "--squash", "split/a")
        r.git("commit", "-q", "-m", "feat: part a (#11)")
        r.git("update-ref", "refs/remotes/origin/master", r.git("rev-parse", "HEAD"))
        r.git("switch", "-q", "master")
        r.run("record", path, "--part", "a", "--landed")
        steps = {s["part"]: s for s in json.loads(r.run("restack", path))["steps"]}
        self.assertEqual(sorted(steps), ["c", "d"])
        step = steps["d"]
        # b still sits on the old base, so d must integrate the base that holds a.
        self.assertEqual(step["onto_ref"], "merge(origin/master, split/b)")
        self.assertEqual(step["commands"][0], "git switch --detach origin/master")
        self.assertEqual(
            step["commands"][1], 'git merge --no-ff -m "chore: integrate master, b for d" refs/heads/split/b'
        )
        self.assertIn("push_base", step)
        self.assertNotIn("retarget", step)
        self.assertIn("merge of `master`, #12", r.run("stack", path, "--part", "d"))
        for pid in ("c", "d"):
            for command in steps[pid]["commands"]:
                r.git(*shlex.split(command)[1:])
            r.git("switch", "-q", "master")
        r.run("record", path, "--part", "c")
        code, _out, err = r.run_status("record", path, "--part", "d")
        self.assertEqual(code, 2)
        self.assertIn("--onto", err)
        r.run("record", path, "--part", "d", "--onto", r.git("rev-parse", "split/d^"))
        self.assertEqual(r.git("show", "split/d:a.txt"), "a1\na2\na3\na4\nA5")
        self.assertEqual(r.git("show", "split/d:b.txt"), "b1\nb2\nb3\nb4\nB5")
        self.assertEqual(json.loads(r.run("restack", path))["status"], "current")
        # An unrelated base move does not make the integration stale.
        r.git("switch", "-q", "--detach", "origin/master")
        r.write("core.txt", "core moved\n")
        r.commit("chore: unrelated")
        r.git("update-ref", "refs/remotes/origin/master", r.git("rev-parse", "HEAD"))
        r.git("switch", "-q", "master")
        self.assertEqual(json.loads(r.run("restack", path))["status"], "current")

    def test_partial_fan_in_keeps_waiting_under_wait_policy(self) -> None:
        r = self.repo
        data = self.mixed()
        for raw, number in zip(data["parts"], (11, 12, 13, 14)):
            raw["pr"] = number
        data["parts"][0]["landed"] = True
        path = self.write_plan(data)
        code, _out, err = r.run_status("stack", path, "--part", "d")
        self.assertEqual(code, 2)
        self.assertIn("multi_parent=wait", err)

    def test_stack_lines(self) -> None:
        r = self.repo
        data = self.mixed()
        for raw, number in zip(data["parts"], (11, 12, 13, 14)):
            raw["pr"] = number
        path = self.write_plan(data)
        self.assertEqual(
            r.run("stack", path, "--part", "c").strip(),
            "- **Stack**: part 3/4 of the `feat/source` split (mixed); base `split/a`; depends on #11; merge after it.",
        )
        self.assertIn("Stacked on this: #13, #14.", r.run("stack", path, "--part", "a"))
        code, _out, err = r.run_status("stack", path, "--part", "d")
        self.assertEqual(code, 2)
        self.assertIn("multi_parent=wait", err)
        merge_policy = copy.deepcopy(data)
        merge_policy["multi_parent"] = "merge"
        path = self.write_plan(merge_policy)
        self.assertIn("base `split/d--base` (merge of #11, #12)", r.run("stack", path, "--part", "d"))
        parallel = plan_data(r, [part("a", [r.c1, r.c3]), part("b", [r.c2, r.c4])])
        path = self.write_plan(parallel)
        self.assertEqual(
            r.run("stack", path, "--part", "b").strip(),
            "- **Stack**: independent part 2/2 of the `feat/source` split; base `master`; merges in any order.",
        )

    def test_coverage_catches_a_dropped_commit(self) -> None:
        r = self.repo
        data = plan_data(r, [part("a", [r.c1, r.c3]), part("b", [r.c2, r.c4])])
        path = self.write_plan(data)
        r.build("split/a", r.base, r.c1, r.c3)
        r.build("split/b", r.base, r.c2)  # c4 forgotten
        r.run("record", path, "--part", "a")
        r.run("record", path, "--part", "b")
        code, out, _err = r.run_status("coverage", path)
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out)["differs"], ["a.txt", "b.txt"])

    def test_coverage_with_left_on_source_still_catches_a_dropped_hunk(self) -> None:
        r = self.repo
        data = plan_data(r, [part("a", [r.c1, r.c3]), part("b", [r.c2])], left=[r.c4])
        path = self.write_plan(data)
        r.build("split/a", r.base, r.c1)  # c3 forgotten; c4 is left and also edits a.txt
        r.build("split/b", r.base, r.c2)
        r.run("record", path, "--part", "a")
        r.run("record", path, "--part", "b")
        code, out, _err = r.run_status("coverage", path)
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out), {"status": "fail", "differs": ["a.txt"]})

    def test_coverage_with_left_on_source_rejects_an_extra_file(self) -> None:
        r = self.repo
        data = plan_data(r, [part("a", [r.c1, r.c3]), part("b", [r.c2])], left=[r.c4])
        path = self.write_plan(data)
        r.build("split/a", r.base, r.c1, r.c3)
        r.build("split/b", r.base, r.c2)
        r.run("record", path, "--part", "a")
        r.run("record", path, "--part", "b")
        out = json.loads(r.run("coverage", path))
        self.assertEqual(out, {"status": "review-left-on-source", "differs": ["a.txt", "b.txt"]})
        r.git("switch", "-q", "split/b")
        r.write("x.txt", "stray\n")
        r.commit("chore: stray file")
        r.git("switch", "-q", "master")
        code, out, _err = r.run_status("coverage", path)
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out), {"status": "fail", "differs": ["x.txt"]})

    def test_coverage_catches_a_hunk_duplicated_by_hunk_split_parts(self) -> None:
        r = self.repo
        # c2 is shared: a takes none of its hunks, b takes its only hunk.
        data = plan_data(r, [part("a", [r.c1, r.c2]), part("b", [r.c2])], left=[r.c3, r.c4])
        path = self.write_plan(data)
        r.build("split/a", r.base, r.c1)
        r.build("split/b", r.base, r.c2)
        r.run("record", path, "--part", "a")
        r.run("record", path, "--part", "b")
        self.assertEqual(json.loads(r.run("coverage", path))["status"], "review-left-on-source")
        r.git("switch", "-q", "split/a")
        r.git("cherry-pick", r.c2)  # the same hunk now sits in both parts
        r.git("switch", "-q", "master")
        r.run("record", path, "--part", "a")
        code, out, _err = r.run_status("coverage", path)
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out), {"status": "fail", "differs": ["b.txt"]})

    def test_coverage_catches_a_tree_entry_duplicated_by_hunk_split_parts(self) -> None:
        r = self.repo
        # An added empty file and a mode-only change have no text lines, so a line balance
        # cannot see two parts carrying them twice.
        r.git("switch", "-q", "feat/source")
        r.write("empty.txt", "")
        # Windows chmod cannot set the exec bit (core.filemode=false), so stage the mode in
        # the index; the chmod keeps POSIX's `add -A` in commit() from restaging 644.
        (r.root / "core.txt").chmod(0o755)
        r.git("add", "-A")
        r.git("update-index", "--chmod=+x", "core.txt")
        c5 = r.commit("chore: empty file and mode")
        r.source = c5
        r.git("switch", "-q", "master")
        data = plan_data(r, [part("a", [r.c1, c5]), part("b", [r.c2, c5])], left=[r.c3, r.c4])
        path = self.write_plan(data)
        r.build("split/a", r.base, r.c1, c5)
        r.build("split/b", r.base, r.c2, c5)  # all of c5 lands in both parts
        r.run("record", path, "--part", "a")
        r.run("record", path, "--part", "b")
        code, out, _err = r.run_status("coverage", path)
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out), {"status": "fail", "differs": ["core.txt", "empty.txt"]})

    def test_coverage_catches_a_hunk_built_into_the_wrong_part(self) -> None:
        r = self.repo
        data = plan_data(r, [part("a", [r.c1]), part("b", [r.c2, r.c3], ["a"])], left=[r.c4])
        path = self.write_plan(data)
        r.build("split/a", r.base, r.c1, r.c3)  # b's c3 hunk built into a
        r.build("split/b", "split/a", r.c2)
        r.run("record", path, "--part", "a")
        r.run("record", path, "--part", "b")
        code, out, _err = r.run_status("coverage", path)
        self.assertEqual(code, 1)
        self.assertEqual(json.loads(out), {"status": "fail", "differs": ["a.txt"]})


if __name__ == "__main__":
    unittest.main()
