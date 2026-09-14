#!/usr/bin/env python3
"""验证 execute-plan 已提交树与远端同名分支精确落地。"""

from __future__ import annotations

import hashlib
import subprocess
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from execute_plan_common import (  # noqa: E402
    ExecutePlanRuntimeError,
    atomic_json,
    load_json,
    relative,
    save_state,
)
from execute_plan_finalization import verified_worktree_tree  # noqa: E402
from execution_baseline import (  # noqa: E402
    validate_existing,
    validate_owner_disjoint,
)
from task_graph_parser import parse_tasks  # noqa: E402
from workspace_manifest import git  # noqa: E402


def verify_landing(
    repo: Path,
    plan: Path,
    paths: dict[str, Path],
) -> str:
    state = load_json(paths["state"])
    if state.get("phase") != "READY_TO_COMMIT":
        raise ExecutePlanRuntimeError("verify-landing 需要 READY_TO_COMMIT")
    baseline_payload = validate_existing(
        repo,
        plan,
        paths["baseline"],
        allow_head_change=True,
    )
    validate_owner_disjoint(baseline_payload, parse_tasks(plan.read_text()))
    evidence = load_json(repo / state["final_evidence"])
    expected_tree = state["verified_worktree_tree"]
    if evidence.get("verified_worktree_tree") != expected_tree:
        raise ExecutePlanRuntimeError("state 与 final evidence tree 不一致")
    head_commit = git(repo, "rev-parse", "HEAD")
    head_tree = git(repo, "rev-parse", "HEAD^{tree}")
    if head_tree != expected_tree:
        raise ExecutePlanRuntimeError("HEAD tree 不是已验证 tree")
    if verified_worktree_tree(repo, baseline_payload) != head_tree:
        raise ExecutePlanRuntimeError("commit 后仍有计划 delta 未落盘或 tree 已漂移")
    start_head = str(baseline_payload["execution_start_head"])
    result = subprocess.run(
        [
            "git",
            "-C",
            str(repo),
            "merge-base",
            "--is-ancestor",
            start_head,
            head_commit,
        ],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise ExecutePlanRuntimeError("landing HEAD 不包含 execution start HEAD")
    branch = git(repo, "branch", "--show-current")
    if not branch:
        raise ExecutePlanRuntimeError("landing 后不得处于 detached HEAD")
    # Branch authorization is owned by commit-and-push. Bus permits an explicit
    # developer request to land directly on master, so this proof checks only
    # that a named branch was pushed exactly.
    remote = git(repo, "ls-remote", "--heads", "origin", f"refs/heads/{branch}")
    remote_head = remote.split()[0] if remote else ""
    if remote_head != head_commit:
        raise ExecutePlanRuntimeError("远端同名分支未精确包含本地 HEAD")
    proof = {
        "schema_version": 1,
        "kind": "execute-plan-landing-proof",
        "plan": state["plan"],
        "head": head_commit,
        "tree": head_tree,
        "branch": branch,
        "remote_head": remote_head,
        "final_evidence": state["final_evidence"],
        "final_evidence_hash": hashlib.sha256(
            (repo / state["final_evidence"]).read_bytes()
        ).hexdigest(),
    }
    atomic_json(paths["landing"], proof)
    state["phase"] = "COMPLETE"
    state["landing_proof"] = relative(repo, paths["landing"])
    save_state(paths["state"], state)
    # 本仓只在 feature 分支落盘（AGENTS.md「Git 规范」），landing 后一律走人工验证 + /review-pr + PR。
    next_step = "developer manual verification, then /review-pr"
    return (
        "EXECUTE_PLAN_STATUS\n"
        "STATE=COMPLETE\n"
        f"HEAD={head_commit}\n"
        f"TREE={head_tree}\n"
        f"EVIDENCE={state['final_evidence']},{relative(repo, paths['landing'])}\n"
        f"MANUAL_VERIFICATION={evidence['manual_e2e']}\n"
        f"NEXT={next_step}\n"
    )
