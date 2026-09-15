#!/usr/bin/env python3
"""Map a TRUSTED_ROOM_ASSIGNMENT_V1 signal to one shared skill context.

The production verifier owns every trust decision. This consumer only checks whether a
Bus assignment signal is present, invokes the verifier when the input is complete, and
decodes its typed JSON result. It never reads assignment storage or compares identities.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Mapping, Sequence, Union


BUS_DISCOVERY_VARIABLES = (
    "BUS_BINARY",
    "BUS_TRUSTED_ASSIGNMENT_DIR",
    "BUS_TRUSTED_ASSIGNMENT_ENDPOINT",
    "BUS_TRUSTED_ASSIGNMENT_TOKEN",
)
REQUIRED_BUS_DISCOVERY_VARIABLES = frozenset(BUS_DISCOVERY_VARIABLES)
# Guards against a hung verifier process; the verifier itself only reads local discovery files.
VERIFIER_TIMEOUT_SECONDS = 30.0
NOT_IN_BUS_ROOM = "NotInBusRoom"
VERIFIED = "verified"


@dataclass(frozen=True)
class StandaloneContext:
    origin: str = NOT_IN_BUS_ROOM


@dataclass(frozen=True)
class OrchestratedContext:
    goal: str
    non_goals: str
    assignment: Mapping[str, object] = field(default_factory=dict)
    origin: str = VERIFIED


@dataclass(frozen=True)
class BlockedContext:
    reason: str
    origin: str = "blocked"


AssignmentContext = Union[StandaloneContext, OrchestratedContext, BlockedContext]


def present_bus_discovery_variables(env: Mapping[str, str]) -> frozenset[str]:
    return frozenset(name for name in BUS_DISCOVERY_VARIABLES if name in env)


def run_production_verifier(
    frame: str,
    env: Mapping[str, str],
    *,
    timeout: float = VERIFIER_TIMEOUT_SECONDS,
) -> str | BlockedContext:
    argv = [env["BUS_BINARY"], "--bus", "assignment", "verify", "--frame", frame]
    try:
        completed = subprocess.run(
            argv,
            env=dict(env),
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return BlockedContext(reason="verifier-timeout")
    except OSError:
        return BlockedContext(reason="verifier-unavailable")
    if completed.returncode != 0:
        return BlockedContext(reason=f"verifier-exit-{completed.returncode}")
    return completed.stdout


def map_verifier_result(result: str | BlockedContext) -> AssignmentContext:
    if isinstance(result, BlockedContext):
        return result
    try:
        payload = json.loads(result)
    except json.JSONDecodeError:
        return BlockedContext(reason="verifier-malformed")
    if not isinstance(payload, dict):
        return BlockedContext(reason="verifier-malformed")
    status = payload.get("status")
    if status == "invalid" and set(payload) == {"status", "reason"}:
        reason = payload["reason"]
        if isinstance(reason, str) and reason:
            return BlockedContext(reason=f"verifier-invalid:{reason}")
        return BlockedContext(reason="verifier-malformed")
    if status == "absent":
        # The CLI requires --frame, so a frame-bearing call can never legitimately be absent.
        return BlockedContext(reason="verifier-absent-unreachable")
    if status != VERIFIED or set(payload) != {"status", "assignment"}:
        return BlockedContext(reason="verifier-malformed")
    assignment = payload["assignment"]
    if not isinstance(assignment, dict):
        return BlockedContext(reason="verifier-malformed")
    goal = assignment.get("goal")
    non_goals = assignment.get("non_goals")
    if not (
        isinstance(goal, str)
        and goal.strip()
        and isinstance(non_goals, str)
        and non_goals.strip()
    ):
        return BlockedContext(reason="verifier-malformed")
    return OrchestratedContext(goal=goal, non_goals=non_goals, assignment=assignment)


def load_assignment_context(
    frame: str | None,
    env: Mapping[str, str],
    *,
    timeout: float = VERIFIER_TIMEOUT_SECONDS,
) -> AssignmentContext:
    frame = frame or None
    present = present_bus_discovery_variables(env)
    if frame is None and not present:
        return StandaloneContext()
    if (
        frame is None
        or present != REQUIRED_BUS_DISCOVERY_VARIABLES
        or not all(env[name] for name in BUS_DISCOVERY_VARIABLES)
    ):
        return BlockedContext(reason="incomplete-bus-intent")
    return map_verifier_result(run_production_verifier(frame, env, timeout=timeout))


def render_context(context: StandaloneContext | OrchestratedContext) -> str:
    if isinstance(context, OrchestratedContext):
        payload: dict[str, object] = {
            "origin": VERIFIED,
            "goal": context.goal,
            "non_goals": context.non_goals,
            "assignment": dict(context.assignment),
        }
    else:
        payload = {"origin": NOT_IN_BUS_ROOM}
    return json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def read_assignment_context(path: Path) -> StandaloneContext | OrchestratedContext:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload == {"origin": NOT_IN_BUS_ROOM}:
        return StandaloneContext()
    if (
        isinstance(payload, dict)
        and set(payload) == {"origin", "goal", "non_goals", "assignment"}
        and payload["origin"] == VERIFIED
        and isinstance(payload["goal"], str)
        and payload["goal"].strip()
        and isinstance(payload["non_goals"], str)
        and payload["non_goals"].strip()
        and isinstance(payload["assignment"], dict)
    ):
        return OrchestratedContext(
            goal=payload["goal"],
            non_goals=payload["non_goals"],
            assignment=payload["assignment"],
        )
    raise ValueError(f"not a room assignment context: {path}")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Resolve the shared room assignment context for a canonical skill"
    )
    parser.add_argument("--frame")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    context = load_assignment_context(args.frame, os.environ)
    if isinstance(context, BlockedContext):
        args.output.unlink(missing_ok=True)
        print("ASSIGNMENT_ORIGIN=blocked")
        print(f"REASON={context.reason}")
        return 1
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(render_context(context), encoding="utf-8")
    print(f"ASSIGNMENT_ORIGIN={context.origin}")
    print(f"ASSIGNMENT_CONTEXT_FILE={args.output}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
