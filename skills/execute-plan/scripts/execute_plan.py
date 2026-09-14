#!/usr/bin/env python3
"""execute-plan 幂等高层 driver：分配 action、消费 evidence、验证最终树。"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parents[2]
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from execute_plan_common import repo_plan  # noqa: E402
from final_gate_contract import GATE_ORDER  # noqa: E402


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=str(REPO_ROOT))
    commands = parser.add_subparsers(dest="command", required=True)
    start = commands.add_parser("start")
    start.add_argument("--plan", required=True)
    for name in ("status", "next"):
        command = commands.add_parser(name)
        command.add_argument("--plan", required=True)
    for name in ("ingest-completion", "ingest-review"):
        command = commands.add_parser(name)
        command.add_argument("--plan", required=True)
        command.add_argument("--action-id", required=True)
        command.add_argument("--expected-state-hash", required=True)
    run_final = commands.add_parser("run-final")
    run_final.add_argument("--plan", required=True)
    # choices 从契约的有序 SOT 派生，不再单独硬编码一份：契约扩了 kind 而这里没跟上时，
    # 新槽位会「声明得了、执行不了」——正是本 PR 要消灭的失败模式。
    run_final.add_argument(
        "--kind",
        choices=GATE_ORDER,
        required=True,
    )
    run_final.add_argument("final_command", nargs=argparse.REMAINDER)
    finalize = commands.add_parser("finalize")
    finalize.add_argument("--plan", required=True)
    finalize.add_argument("--require-build", action="store_true")
    finalize.add_argument("--manual-e2e", action="store_true")
    landing = commands.add_parser("verify-landing")
    landing.add_argument("--plan", required=True)
    resume = commands.add_parser("resume-repair")
    resume.add_argument("--plan", required=True)
    resume.add_argument("--context")
    return parser


def _dispatch(args: argparse.Namespace) -> str:
    repo = Path(args.repo).resolve()
    plan, plan_relative = repo_plan(repo, args.plan)
    if args.command == "start":
        from execute_plan_lifecycle import start_execution

        return start_execution(
            repo,
            plan,
            plan_relative,
        )

    from execute_plan_common import load_json, runtime_paths
    from execute_plan_lifecycle import (
        next_action,
        status,
        validate_execution_context,
    )

    paths = runtime_paths(repo, plan)
    if args.command not in {"run-final", "finalize", "verify-landing"}:
        validate_execution_context(repo, plan, paths)
    if args.command == "status":
        return status(repo, plan)
    if args.command == "next":
        return next_action(
            repo,
            plan,
            plan_relative,
            paths,
            load_json(paths["state"]),
        )
    if args.command in {"run-final", "finalize", "verify-landing"}:
        from execute_plan_finalization import (
            finalize,
            run_final_gate,
        )
        from execute_plan_landing import verify_landing

        if args.command == "run-final":
            command = tuple(args.final_command)
            if command[:1] == ("--",):
                command = command[1:]
            return run_final_gate(
                repo,
                plan,
                paths,
                gate_kind=args.kind,
                command=command,
            )
        if args.command == "finalize":
            return finalize(
                repo,
                plan,
                paths,
                require_build=args.require_build,
                manual_e2e=args.manual_e2e,
            )
        return verify_landing(repo, plan, paths)
    from execute_plan_acceptance import ingest_completion, ingest_review
    from execute_plan_recovery import resume_repair

    if args.command == "ingest-completion":
        return ingest_completion(
            repo,
            plan,
            plan_relative,
            action_id=args.action_id,
            expected_state_hash=args.expected_state_hash,
        )
    if args.command == "resume-repair":
        return resume_repair(
            repo,
            plan,
            context=args.context,
        )
    return ingest_review(
        repo,
        plan,
        plan_relative,
        action_id=args.action_id,
        expected_state_hash=args.expected_state_hash,
    )


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        print(_dispatch(args), end="")
        return 0
    except (OSError, UnicodeError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
