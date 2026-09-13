#!/usr/bin/env python3
"""Opt-in live-provider acceptance through a running Bus --dev domain interface.

No PTY/UI input, fabricated callbacks, provider bypass flags, or credential reads.
Creates one uniquely named test room; leaves it available for inspection.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


def verify_delivery(status, expected, token):
    assert status["complete"], status
    requests = status["requests"]
    assert {r["agent_id"] for r in requests} == set(expected), requests
    assert len(requests) == len(expected), requests
    for request in requests:
        assert request["stage"] == "replied", request
        assert request["start_bound"] and request["session_id"], request
        assert request["reply"]["text"].strip() == token, request


def profiles():
    return {
        "claude-codex": {"providers": ("claude", "codex", "claude", "codex"), "messages": 20,
                         "recipients": ((0,), (1,), (0, 1), (0, 1, 2), (0, 1, 2, 3))},
        "claude-codex-cursor": {"providers": ("claude", "codex", "cursor"), "messages": 15,
                                "recipients": ((0,), (1,), (2,), (0, 1), (0, 2), (1, 2), (0, 1, 2))},
    }


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-live-models", action="store_true", required=True)
    parser.add_argument("--profile", choices=profiles(), default="claude-codex")
    parser.add_argument("--claude-pwd", required=True)
    parser.add_argument("--codex-pwd", required=True)
    parser.add_argument("--cursor-pwd")
    parser.add_argument("--messages", type=int)
    args = parser.parse_args(argv)
    if "cursor" in profiles()[args.profile]["providers"] and not args.cursor_pwd:
        parser.error("--cursor-pwd is required for the claude-codex-cursor profile")
    if args.messages is None:
        args.messages = profiles()[args.profile]["messages"]
    if not 1 <= args.messages <= 100:
        parser.error("--messages must be between 1 and 100")
    return args


def main():
    args = parse_args()
    data = Path(os.environ.get("BUS_DATA_DIR", "")).resolve()
    if not os.environ.get("BUS_DATA_DIR") or data == Path.home() / ".local/share/bus":
        raise SystemExit("Set BUS_DATA_DIR to a disposable, already running Bus --dev instance")
    run_id = uuid.uuid4().hex[:12]
    artifact = ROOT / "temp/artifacts" / ("bus-dev-" + run_id)
    artifact.mkdir(parents=True)
    profile = profiles()[args.profile]
    evidence = {"data_root": str(data), "run_id": run_id, "profile": args.profile, "cases": [], "passed": False}

    def save():
        (artifact / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")

    def bus(*argv, timeout=30):
        result = subprocess.run([str(ROOT / "bus"), *map(str, argv)], capture_output=True,
                                text=True, timeout=timeout, check=False)
        try:
            response = json.loads(result.stdout)
        except ValueError as error:
            raise AssertionError(f"Invalid CLI response: {result.stdout} {result.stderr}") from error
        if result.returncode or not response["ok"]:
            evidence["last_error"] = response
            save()
            raise AssertionError(response)
        return response["result"]

    try:
        bus("state")  # Connect-only; never launch or enable a target implicitly.
        room = bus("room", "create", "dev-acceptance-" + run_id)["room_id"]
        evidence["room_id"] = room
        agents = []
        for provider in profile["providers"]:
            name = provider + str(1 + sum(a["provider"] == provider for a in agents))
            cwd = getattr(args, provider + "_pwd")
            result = bus("agent", "add", "--room", room, "--name", name, "--provider", provider,
                         "--pwd", cwd, "--consent-hooks")
            agents.append({"id": result["agent_id"], "name": name, "provider": provider})
            evidence["agents"] = agents
            save()
        # This confirms Bus setup, not provider trust. Review provider hooks before this run.
        for agent in agents:
            if agent["provider"] in ("codex", "cursor"):
                bus("agent", "setup-confirm", agent["id"], "--confirm")
        deadline = time.monotonic() + 90
        while True:
            diagnostics = bus("diagnostics")
            selected = [a for a in diagnostics["agents"] if a["agent_id"] in {a["id"] for a in agents}]
            if len(selected) == len(agents) and all(a["reason"] is None for a in selected):
                break
            evidence["readiness"] = selected
            if time.monotonic() >= deadline:
                raise AssertionError("Agents not ready; inspect evidence readiness and owned terminals")
            time.sleep(1)
        sets = profile["recipients"]
        for index in range(args.messages):
            recipients = [agents[i] for i in sets[index % len(sets)]]
            token = f"BUS_DEV_{run_id}_{index + 1:02d}"
            text = f"Reply with exactly {token} and nothing else. Do not use tools or modify files."
            selector = "all" if len(recipients) == len(agents) else ",".join(str(a["id"]) for a in recipients)
            receipt = bus("send", "--room", room, "--to", selector, "--text", text,
                          "--request-id", f"acceptance-{run_id}-{index}")
            case = {"token": token, "recipient_ids": [a["id"] for a in recipients], "receipt": receipt}
            evidence["cases"].append(case)
            save()
            status = bus("wait", "--message", receipt["message_id"], "--timeout", 180, timeout=185)
            verify_delivery(status, case["recipient_ids"], token)
            case["status"] = status
            # Inspect actual owned runtime output too: catches unintended terminal fan-out.
            for agent in agents:
                runtime = bus("agent", "read", agent["id"])
                seen = token in json.dumps(runtime["output"])
                assert seen == (agent in recipients), (agent, "unexpected token visibility", seen)
                case.setdefault("terminal_checks", {})[agent["name"]] = {"token_seen": seen}
            case["passed"] = True
            save()
            print(json.dumps({"case": index + 1, "recipients": len(recipients), "passed": True}), flush=True)
        history = bus("history", "--room", room)
        ids = [m["prompt"]["id"] for m in history["messages"]]
        assert len(ids) == args.messages and ids == sorted(ids) and len(set(ids)) == len(ids)
        assert ids == [c["receipt"]["message_id"] for c in evidence["cases"]]
        evidence["history"] = history
        evidence["passed"] = True
        save()
        print(json.dumps({"passed": True, "artifact": str(artifact / "evidence.json")}), flush=True)
    except Exception as error:
        evidence["failure"] = str(error)
        try:
            evidence["diagnostics"] = bus("diagnostics")
        except Exception:
            pass
        save()
        print(json.dumps({"passed": False, "artifact": str(artifact / "evidence.json"), "error": str(error)}), flush=True)
        raise


if __name__ == "__main__":
    main()
