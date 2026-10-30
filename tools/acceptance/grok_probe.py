#!/usr/bin/env python3
"""Validate an isolated native-wake capture without starting Grok.

The CLI only replays JSON evidence. REPLAY_VALID is deliberately distinct from
native PASS. A future authenticated PTY recorder can call run_probe explicitly;
no recorder, login, global configuration or real model turn is invoked here.
All event names below form a normalized *probe* schema, not a claimed native
Grok transcript schema. The recorder must retain the underlying hook/PTY log.
"""
import argparse
import json
import math
from pathlib import Path
import re

PROMPT = (
    "Synthetic native-wake probe. Use run_terminal_command with background:true "
    "to run exactly the command: `sleep 8; printf GROK_WAKE_DONE`. Do not poll, wait, or call "
    "any other tool. Finish this initial turn with exactly GROK_WAITING. After "
    "the background completion starts a new turn, reply exactly GROK_WOKE."
)


def _result(status, reason, **details):
    return {"status": status, "reason": reason, **details}


def evaluate_trace(trace, *, native=False):
    """Fail closed on missing ownership, no idle gap, injected input or notices."""
    if not isinstance(trace, dict):
        return _result("FAIL", "capture must be a JSON object")
    if trace.get("authenticated") is not True:
        return _result("PENDING_AUTH", "Grok is not signed in; native wake remains pending")
    if native and (trace.get("provenance") != "native-pty"
                   or trace.get("synthetic") is not False
                   or not isinstance(trace.get("binary_sha256"), str)
                   or not re.fullmatch(r"[0-9a-f]{64}", trace["binary_sha256"])
                   or not re.fullmatch(r"grok 1[.]0[.]5 [(][0-9a-f]{12,40}[)]", str(trace.get("version", "")))):
        return _result("FAIL", "native PASS requires authenticated version-bound native PTY provenance")
    events = trace.get("events")
    if not isinstance(events, list):
        return _result("FAIL", "events must be a list")
    previous = -1
    for event in events:
        if not isinstance(event, dict):
            return _result("FAIL", "malformed event")
        at = event.get("at")
        if isinstance(at, bool) or not isinstance(at, (int, float)) or not math.isfinite(at) or at < 0 or at < previous:
            return _result("FAIL", "events need ordered finite monotonic timestamps")
        previous = at
        if event.get("kind") in ("cancelled", "failure"):
            return _result("FAIL", "capture contains cancellation or failure")
    keyboard = [e for e in events if e.get("kind") == "keyboard"]
    if len(keyboard) != 1 or (isinstance(keyboard[0].get("bytes"), bool) or not isinstance(keyboard[0].get("bytes"), int)) or keyboard[0]["bytes"] <= 0:
        return _result("FAIL", "require exactly one initial keyboard submission and zero later bytes")
    starts = [e for e in events if e.get("kind") == "turn_start"]
    finals = [e for e in events if e.get("kind") == "turn_final"]
    idle = [e for e in events if e.get("kind") == "idle"]
    exits = [e for e in events if e.get("kind") == "shell_exit"]
    if len(starts) < 2 or len(finals) < 2 or not idle or not exits:
        return _result("TIMEOUT", "no complete first-turn / idle / shell-exit / second-turn evidence")
    if len(starts) != 2 or len(finals) != 2 or len(exits) != 1:
        return _result("FAIL", "ambiguous turn or task boundaries")
    first, second = starts
    done, woke = finals
    shell = exits[0]
    quiet = next((e for e in idle if e.get("turn") == first.get("turn")), {})
    session = first.get("session")
    if not session or not first.get("turn") or not second.get("turn"):
        return _result("FAIL", "missing session/turn identity")
    if any(e.get("session") != session for e in (second, done, woke, shell, quiet)):
        return _result("FAIL", "mixed or missing session provenance")
    if first["turn"] == second["turn"] or done.get("turn") != first["turn"] or woke.get("turn") != second["turn"]:
        return _result("FAIL", "the wake must own a distinct second model turn")
    if (first.get("source") != "keyboard" or second.get("source") != "background_task"
            or not shell.get("task") or second.get("task") != shell["task"]):
        return _result("FAIL", "wake is not attributed to the observed background task")
    launched = shell.get("started_at")
    if (isinstance(launched, bool) or not isinstance(launched, (int, float))
            or not math.isfinite(launched)
            or not first["at"] <= launched < done["at"]
            or shell["at"] - launched < 8
            or shell.get("command") != "sleep 8; printf GROK_WAKE_DONE"
            or shell.get("background") is not True or shell.get("turn") != first["turn"]):
        return _result("FAIL", "require the owned background sleep to start in the first turn and run at least 8s")
    if (shell.get("exit_code") != 0 or str(shell.get("text", "")).strip() != "GROK_WAKE_DONE"
            or done.get("outcome") != "success" or woke.get("outcome") != "success"
            or str(done.get("text", "")).strip() != "GROK_WAITING"
            or str(woke.get("text", "")).strip() != "GROK_WOKE"):
        return _result("FAIL", "missing exact successful sentinel finals or task exit")
    if not (keyboard[0]["at"] < first["at"] < done["at"] <= quiet["at"]
            < shell["at"] <= second["at"] < woke["at"]):
        return _result("FAIL", "require a genuine idle gap before shell completion and a later new turn")
    # A resumed first turn or any work between idle and task exit is polling.
    if any(e.get("kind") in ("working", "tool_start", "turn_start")
           and quiet["at"] < e["at"] < shell["at"] for e in events):
        return _result("FAIL", "the first turn remained active or polled instead of going idle")
    return _result("PASS" if native else "REPLAY_VALID",
                   "distinct owned second turn after idle and shell completion; no later keyboard bytes",
                   idle_gap_s=shell["at"] - quiet["at"], first_turn=first["turn"],
                   wake_turn=second["turn"], session=session)


def run_probe(*, authenticated=False, allow_live_models=False, recorder=None, timeout=90):
    """Invoke an explicitly supplied native recorder once, only with live opt-in.

    recorder(prompt, timeout) owns a scratch PTY, makes ONE keyboard write,
    records native boundaries and hashes the exact executable, then stops it.
    Synthetic/mock recorders must mark their trace synthetic, never native.
    """
    if not authenticated:
        return _result("PENDING_AUTH", "Grok is not signed in; no model was started")
    if not allow_live_models:
        raise ValueError("native capture requires explicit allow_live_models")
    if recorder is None:
        return _result("TIMEOUT", "no native recorder supplied; no model was started")
    try:
        trace = recorder(PROMPT, timeout)
    except TimeoutError:
        return _result("TIMEOUT", "native recorder timed out")
    return evaluate_trace(trace, native=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trace", type=Path, help="Replay a normalized capture; never starts a model")
    args = parser.parse_args(argv)
    if args.trace:
        try:
            result = evaluate_trace(json.loads(args.trace.read_text()))
        except (OSError, ValueError) as error:
            result = _result("FAIL", str(error))
    else:
        result = run_probe()
    print(json.dumps(result, allow_nan=False))
    return 0 if result["status"] in ("PASS", "REPLAY_VALID") else 2


if __name__ == "__main__":
    raise SystemExit(main())
