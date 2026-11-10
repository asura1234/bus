#!/usr/bin/env python3
"""Measure only driver-owned fake providers; never locate a running Bus."""

import argparse
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def wait(predicate, description, timeout=90):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.025)
    raise RuntimeError("timeout " + description)


class ScratchBus:
    def __init__(self, binary, root, name, run, expected_sha):
        self.binary, self.root, self.name, self.run = binary, root, name, run
        self.expected_sha = expected_sha
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("BUS_", "HERDR_"))
        }
        self.env["BUS_TUI_ROOT"] = str(root)
        self.data = None

    def driver(self, *args):
        result = subprocess.run(
            [str(self.binary), "--dev", "tui", *map(str, args)],
            env=self.env,
            capture_output=True,
            text=True,
            timeout=120,
        )
        if result.returncode:
            raise RuntimeError((args, result.stdout, result.stderr))
        return json.loads(result.stdout)

    def control(self, *args):
        value = self.driver("bus", "--name", self.name, *args)
        if "stdout" in value:
            value = json.loads(value["stdout"])
        if not value.get("ok"):
            raise RuntimeError(value)
        return value["result"]

    def state(self):
        value = self.control("state")
        return value.get("state", value)

    def all_status(self, status):
        return all(agent["status"] == status for agent in self.state()["agents"])

    def ready(self, mode):
        status = "idle" if mode == "idle" else "working"
        if not self.all_status(status):
            return False
        saved = self.saved()
        for agent in saved["agents"].values():
            if (not agent["runtime_identity"].get("session_id")
                    or not agent["hook_setup_confirmed"]
                    or agent["session_binding_invalidated"]
                    or agent["deletion_pending"] or agent["dialog"]):
                return False
            current = agent.get("current_request")
            if mode == "idle":
                if current is not None:
                    return False
            else:
                lead = saved["requests"].get(str(current))
                if (not lead or lead["phase"] != "active"
                        or not lead["trusted_start_bound"]
                        or not lead.get("provider_turn_id")
                        or lead.get("pending_final") is not None):
                    return False
        return True

    def start(self, count):
        info = self.driver(
            "start",
            "--name",
            self.name,
            "--run-dir",
            self.run,
            "--size",
            "120x40",
        )
        (self.run / "start.json").write_text(json.dumps(info, indent=2))
        status = self.driver("status", "--name", self.name)
        (self.run / "status.json").write_text(json.dumps(status, indent=2))
        self.data = Path(status["data_dir"])
        assert self.data.is_relative_to(self.root)
        assert self.data.stat().st_mode & 0o777 == 0o700
        build = self.state()["build"]
        for role in ["client", "server"]:
            assert build[role]["binary"] == str(self.binary), (role, build)
            assert build[role]["sha"] == self.expected_sha, (role, build)
        (self.run / "build.json").write_text(json.dumps(build, indent=2))
        for index in range(count):
            self.control(
                "agent",
                "add",
                "--room",
                "bus",
                "--name",
                f"a{index}",
                "--provider",
                "codex",
                "--consent-hooks",
                "--pwd",
                status.get("work_dir", str(self.data.parent / "work")),
            )
        wait(lambda: self.ready("idle"), "bound and settled idle agents")

    def saved(self):
        # Never poll the measured coordinator for timestamps.
        return json.loads((self.data / "state.json").read_text())["state"]

    def confirmed(self, ids):
        state = self.saved()
        rows = [state["requests"].get(str(key)) for key in ids]
        return (
            rows
            if all(
                row and row["latency"]["native"]["native_confirmed_at_ms"] is not None
                for row in rows
            )
            else None
        )

    def send(self, text, recipient="all"):
        sent = self.control("send", "--room", "bus", "--to", recipient, "--text", text)
        return wait(lambda: self.confirmed(sent["request_ids"]), "native confirmations")

    def stop(self):
        return self.driver("stop", "--name", self.name)


def measure(bus, label, count, rounds, traffic, output, samples):
    for mode in ["idle", "busy-steer"]:
        if mode == "busy-steer":
            wait(lambda: bus.ready("idle"), "settlement before seeded busy turn")
            bus.send("fake:work 3600 busy benchmark")
        for round_no in range(rounds):
            wait(lambda: bus.ready(mode), "warm " + mode + " preconditions")
            recipient = "all" if traffic == "broadcast" else f"a{round_no % count}"
            rows = bus.send(f"latency-{label}-{count}-{mode}-{round_no}", recipient)
            if mode == "idle":
                assert all(row["latency"]["ready_at_enqueue"] for row in rows), (
                    "idle samples require an eligible recipient at acceptance"
                )
            if mode == "busy-steer":
                assert all(row.get("steered") for row in rows), (
                    "busy benchmark must prove grouped mid-turn delivery"
                )
            for row in rows:
                latency = row["latency"]
                accepted = latency["accepted_at_ms"]
                confirmed = latency["native"]["native_confirmed_at_ms"]
                samples.append(
                    {
                        "label": label,
                        "traffic": traffic,
                        "agents": count,
                        "mode": mode,
                        "round": round_no,
                        "request": row["id"],
                        "message": row["prompt"]["id"],
                        "accepted": accepted,
                        "native_confirmed": confirmed,
                        "ms": confirmed - accepted,
                        "steered": row.get("steered", False),
                        "latency": latency,
                    }
                )
            (output / "samples.json").write_text(json.dumps(samples, indent=2))
            print(
                label,
                count,
                mode,
                round_no,
                round(
                    statistics.mean(
                        row["latency"]["native"]["native_confirmed_at_ms"]
                        - row["latency"]["accepted_at_ms"]
                        for row in rows
                    )
                ),
                flush=True,
            )
    (bus.run / "final-state.json").write_text(json.dumps(bus.saved(), indent=2))


def summarize(label, counts, samples):
    result = []
    for count in counts:
        for mode in ["idle", "busy-steer"]:
            values = sorted(
                row["ms"]
                for row in samples
                if row["agents"] == count and row["mode"] == mode
            )
            n = len(values)
            result.append(
                {
                    "label": label,
                    "agents": count,
                    "mode": mode,
                    "n": n,
                    "mean": statistics.mean(values),
                    "p50": values[math.ceil(n * 0.5) - 1],
                    "p90": values[math.ceil(n * 0.9) - 1],
                    "p99": values[math.ceil(n * 0.99) - 1],
                    "max": max(values),
                }
            )
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument(
        "--expected-sha", required=True,
        help="Require both runtime roles to match before adding fake agents",
    )
    parser.add_argument("--output", type=Path, default=Path("temp/reports/latency-fix"))
    parser.add_argument("--rounds", type=int, default=40)
    parser.add_argument("--traffic", choices=["single", "broadcast"], default="single")
    parser.add_argument("--counts", nargs="+", type=int, default=[2, 5, 10, 20])
    args = parser.parse_args()
    if len(args.expected_sha) != 40 or any(
        c not in "0123456789abcdef" for c in args.expected_sha
    ):
        parser.error("expected-sha must be a full lowercase source SHA")
    if args.rounds < 1 or any(count < 1 for count in args.counts):
        parser.error("rounds and agent counts must be positive")
    output = args.output.resolve() / args.label
    output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="blat-", dir="/private/tmp"))
    root.chmod(0o700)
    samples = []
    for count in args.counts:
        run = output / f"n{count}"
        run.mkdir(exist_ok=True)
        bus = ScratchBus(args.binary.resolve(), root, f"n{count}", run, args.expected_sha)
        try:
            bus.start(count)
            print(args.label, count, "started", flush=True)
            measure(bus, args.label, count, args.rounds, args.traffic, output, samples)
        finally:
            stopped = bus.stop()
            (run / "stop.json").write_text(json.dumps(stopped, indent=2))
    summary = summarize(args.label, args.counts, samples)
    (output / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
