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


def round_trip_prompt(token):
    return ("We are only testing Bus message round trip. "
            "Do not use skills, tools, or modify files. Do not perform project work. "
            f"Reply with exactly {token} and nothing else.")


def read_agent_output(bus, agent_id, timeout=15):
    deadline = time.monotonic() + timeout
    while True:
        try:
            output = bus("agent", "read", agent_id, "--source", "recent", "--lines", "80")["text"]
            return output if isinstance(output, str) else json.dumps(output)
        except AssertionError as error:
            transient = "resource temporarily unavailable" in str(error).lower()
            if not transient or time.monotonic() >= deadline:
                raise
            time.sleep(0.25)


def verify_setup_gate(status, expected, token, outputs):
    assert not status["complete"], status
    requests = status["requests"]
    assert len(requests) == len(expected) and {r["agent_id"] for r in requests} == set(expected), status
    for request in requests:
        assert request["stage"] == "queued" and request["reason"] == "hook_setup_unconfirmed", request
        assert not any(request.get(key) for key in ("start_bound", "session_id", "reply", "uncertain_outcome")), request
    for agent, output in outputs.items():
        assert token not in json.dumps(output), (agent, "setup gate leaked prompt into terminal")


def recipient_selector(bus, room, agents, recipients):
    owned = {a["id"] for a in agents}
    selected = {a["id"] for a in recipients}
    assert selected and selected <= owned, "Recipient is not owned by this acceptance run"
    if selected == owned:
        members = {a["id"] for a in bus("state")["agents"] if a["room_id"] == room}
        assert members == owned, ("Test room membership changed before --to all", members, owned)
        return "all"
    return ",".join(str(a["id"]) for a in recipients)


def observe_statuses(samples, diagnostics, agents, recipients, elapsed_ms):
    providers = {a["id"]: a["provider"] for a in agents if a["id"] in recipients}
    latest = {sample["agent_id"]: sample["status"] for sample in samples}
    for agent in diagnostics["agents"]:
        agent_id, status = agent["agent_id"], agent["status"]
        if agent_id in providers and latest.get(agent_id) != status:
            samples.append({"agent_id": agent_id, "provider": providers[agent_id],
                            "status": status, "elapsed_ms": elapsed_ms})


def verify_status_coverage(cases, agents):
    providers = {a["id"]: a["provider"] for a in agents}
    coverage = {provider: set() for provider in providers.values()}
    for case in cases:
        for sample in case.get("status_samples", []):
            if sample["agent_id"] in providers:
                coverage[providers[sample["agent_id"]]].add(sample["status"])
    missing = {provider: sorted({"working", "idle"} - states) for provider, states in coverage.items()
               if not {"working", "idle"} <= states}
    assert not missing, ("Required provider statuses were not observed", missing)
    return {provider: sorted(states) for provider, states in coverage.items()}


def poll_delivery(bus, case, agents, save, timeout=180):
    started = time.monotonic()
    samples = case.setdefault("status_samples", [])
    while True:
        remaining = timeout - (time.monotonic() - started)
        if remaining <= 0:
            raise AssertionError("Delivery or recipient idle observation timed out")
        status = bus("message", "status", case["receipt"]["message_id"], timeout=min(30, remaining))
        case["status"] = status
        assert {r["request_id"] for r in status["requests"]} == set(case["receipt"]["request_ids"]), status
        diagnostics = bus("diagnostics", timeout=min(30, max(0.1, timeout - (time.monotonic() - started))))
        observe_statuses(samples, diagnostics, agents, case["recipient_ids"],
                         round((time.monotonic() - started) * 1000))
        save()
        if status["complete"]:
            verify_delivery(status, case["recipient_ids"], case["token"])
            selected = [a for a in diagnostics["agents"] if a["agent_id"] in case["recipient_ids"]]
            if len(selected) == len(case["recipient_ids"]) and all(a["status"] == "idle" for a in selected):
                return status
        time.sleep(0.2)


def validate_data_root(value, allow_existing_user_root):
    data = Path(value).resolve()
    if not value or (data == (Path.home() / ".local/share/bus").resolve() and not allow_existing_user_root):
        raise SystemExit("Set BUS_DATA_DIR to a running Bus --dev instance; the default user root requires --allow-existing-user-root")
    return data


def profiles():
    return {
        "claude-codex": {"providers": ("claude", "codex", "claude", "codex"), "messages": 20,
                         "recipients": ((0,), (1,), (0, 1), (0, 1, 2), (0, 1, 2, 3))},
        "claude-codex-cursor": {"providers": ("claude", "codex", "cursor"), "messages": 20,
                                "recipients": ((0,), (1,), (2,), (0, 1), (0, 2), (1, 2), (0, 1, 2))},
        "six-agents": {"providers": ("claude", "codex", "cursor", "claude", "codex", "cursor"),
                       "messages": 20,
                       "recipients": ((2,), (0,), (1,), (3,), (4,), (5,),
                                      (0, 3), (1, 4), (2, 5), (0, 1), (1, 2), (2, 3),
                                      (0, 1, 2), (3, 4, 5), (0, 4, 5), (3, 1, 2), (0, 3, 1), (2, 5, 4),
                                      (0, 1, 2, 3, 4, 5), (0, 1, 2, 3, 4, 5))},
    }


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--allow-live-models", action="store_true", required=True)
    parser.add_argument("--allow-existing-user-root", action="store_true",
                        help="Explicitly allow the existing default Bus root; only create a new test room and its agents")
    parser.add_argument("--profile", choices=profiles(), default="claude-codex")
    parser.add_argument("--claude-pwd", required=True)
    parser.add_argument("--codex-pwd", required=True)
    parser.add_argument("--cursor-pwd")
    parser.add_argument("--messages", type=int)
    parser.add_argument("--message-delay", type=float, default=5.0,
                        help="Seconds to wait between completed message cases (default: 5)")
    args = parser.parse_args(argv)
    if "cursor" in profiles()[args.profile]["providers"] and not args.cursor_pwd:
        parser.error("--cursor-pwd is required for a profile containing Cursor")
    if args.profile == "six-agents" and any(Path(getattr(args, provider + "_pwd")).resolve() != ROOT
                                             for provider in ("claude", "codex", "cursor")):
        parser.error("six-agents requires every provider PWD to be " + str(ROOT))
    if args.messages is None:
        args.messages = profiles()[args.profile]["messages"]
    if not 1 <= args.messages <= 100:
        parser.error("--messages must be between 1 and 100")
    if not 0 <= args.message_delay <= 300:
        parser.error("--message-delay must be between 0 and 300 seconds")
    return args


def main():
    args = parse_args()
    data = validate_data_root(os.environ.get("BUS_DATA_DIR", ""), args.allow_existing_user_root)
    run_id = uuid.uuid4().hex[:12]
    artifact = ROOT / "temp/artifacts" / ("bus-dev-" + run_id)
    artifact.mkdir(parents=True)
    profile = profiles()[args.profile]
    evidence = {"data_root": str(data), "run_id": run_id, "profile": args.profile, "cases": [], "passed": False,
                "allow_existing_user_root": args.allow_existing_user_root}

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
        sets = profile["recipients"]

        def send_case(index):
            recipients = [agents[i] for i in sets[index % len(sets)]]
            token = f"BUS_DEV_{run_id}_{index + 1:02d}"
            text = round_trip_prompt(token)
            # Exercise --to all only after checking the new room's exact owned membership.
            selector = recipient_selector(bus, room, agents, recipients)
            receipt = bus("send", "--room", room, "--to", selector, "--text", text,
                          "--request-id", f"acceptance-{run_id}-{index}")
            case = {"token": token, "recipient_ids": [a["id"] for a in recipients], "receipt": receipt}
            evidence["cases"].append(case)
            save()
            return case

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
        for index in range(args.messages):
            recipients = [agents[i] for i in sets[index % len(sets)]]
            case = send_case(index)
            token = case["token"]
            status = poll_delivery(bus, case, agents, save)
            case["status"] = status
            # Inspect actual owned runtime output too: catches unintended terminal fan-out.
            for agent in agents:
                output = read_agent_output(bus, agent["id"])
                seen = token in output
                assert seen == (agent in recipients), (agent, "unexpected token visibility", seen)
                case.setdefault("terminal_checks", {})[agent["name"]] = {"token_seen": seen}
            case["passed"] = True
            save()
            print(json.dumps({"case": index + 1, "recipients": len(recipients), "passed": True}), flush=True)
            if index + 1 < args.messages:
                time.sleep(args.message_delay)
        history = bus("history", "--room", room)
        ids = [m["prompt"]["id"] for m in history["messages"]]
        assert len(ids) == args.messages and ids == sorted(ids) and len(set(ids)) == len(ids)
        assert ids == [c["receipt"]["message_id"] for c in evidence["cases"]]
        evidence["history"] = history
        if args.profile == "six-agents":
            evidence["status_coverage"] = verify_status_coverage(evidence["cases"], agents)
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
