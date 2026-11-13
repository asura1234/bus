#!/usr/bin/env python3
"""Opt-in outbound-only Feishu acceptance through the private TUI driver."""
import argparse
import json
import os
from pathlib import Path
import traceback

if __package__:
    from . import notification_driver as helper
else:
    import notification_driver as helper

DOC = "tests/e2e-tui/feishu-notify.md"


class Run(helper.Run):
    doc = DOC

    def durable(self):
        return json.loads((Path(self.session["data_dir"]) / "state.json").read_text())["state"]

    def post(self, room, sender, text):
        return self.bus("send", "--room", room, "--as", sender, "--to", "you", "--text", text)["result"]["message_id"]

    def edit(self, hit, text):
        self.command("click", "--hit", hit)
        self.command("press", "ctrl+a", "ctrl+k")
        if text:
            self.command("type", text)
        self.command("press", "enter")

    def exercise(self):
        self.session = self.command("start", "--size", "160x55", "--graphics", "none", "--run-dir", self.root / "sessions")
        self.report.write_text(self.report.read_text().replace("- Session: `none`", f"- Session: `{self.session['run_dir']}`"))
        head = (self.root / "candidate.txt").read_text().splitlines()[0].split()[1]
        build = self.state()["build"]
        for role in ("client", "server"):
            assert build[role]["sha"] == head and Path(build[role]["binary"]).resolve() == self.binary, build
        (self.root / "build.json").write_text(json.dumps(build, indent=2))
        work = Path(self.session["data_dir"]).parent / "work"
        agents = []
        for room, name in (("feishu-a", "feishu-orch-a"), ("feishu-b", "feishu-orch-b")):
            self.bus("room", "create", room)
            agents.append(self.bus("agent", "add", "--room", "master", "--name", name, "--provider", "claude",
                                   "--pwd", work, "--orchestrates", room, "--consent-hooks")["result"]["agent_id"])
        worker = self.bus("agent", "add", "--room", "feishu-a", "--name", "feishu-worker", "--provider", "claude",
                          "--pwd", work, "--consent-hooks")["result"]["agent_id"]
        self.until(lambda: all(agent["status"] == "idle" for agent in self.state()["agents"]), "fake agents idle")
        self.command("press", "f6")
        self.command("click", "--hit", "settings")
        self.command("wait", "--text", "Feishu notifications (on)")
        screen = str(self.command("snapshot"))
        assert "Recipient open_id:" in screen and "feishu-bridge path: PATH" in screen
        self.capture("settings-outbound")
        self.command("press", "esc")
        self.post("master", agents[0], "Routine progress")
        self.quiet()
        assert not self.calls()
        self.step(1, "Both exact-head runtime roles checked; private fake agents and PATH bridge enabled; settings screenshot retained; untagged progress did not invoke the bridge")

        sources = [self.post("master", agents[0], "[waiting-on-you] First decision 界 $HOME"),
                   self.post("master", agents[1], "[waiting_on_you] Second decision"),
                   self.post("feishu-a", worker, "[waiting-on-you] Same decision")]
        self.until(lambda: sum(r["message_id"] is not None for r in self.durable()["feishu"]["outbox"].values()) == 3,
                   "initial DMs confirmed before suppressing a repeat")
        sources.append(self.post("feishu-a", worker, "[waiting-on-you] Same decision"))
        self.until(lambda: len(self.calls()) == 3, "four requests share three DMs")
        self.until(lambda: sum(r["message_id"] is not None for r in self.durable()["feishu"]["outbox"].values()) == 3,
                   "returned message/chat mappings committed")
        self.quiet()
        records = self.bridge_records()
        assert len(records) == 3
        saved = self.durable()["feishu"]["outbox"]
        by_source = {route["source"]["prompt_id"]: (mapping, route)
                     for mapping in saved.values() for route in [mapping, *mapping.get("aliases", [])]}
        assert set(by_source) == set(sources)
        assert len({r["outbox"]["key"] for r in records}) == len({r["outbox"]["code"] for r in records}) == 3
        assert "Second decision" not in by_source[sources[0]][0]["text"]
        assert by_source[sources[2]][0] == by_source[sources[3]][0]
        for source in sources[2:]:
            assert by_source[source][1]["raising_agent"] == worker
            assert by_source[source][1]["orchestrator"] == agents[0]
        for record in records:
            mapping = saved[record["outbox"]["code"]]
            assert mapping["message_id"] == record["message_id"] and mapping["chat_id"] == "oc_dm"
        self.assert_bridge()
        self.step(2, "Three v1 sends retain four durable logical routes; identical worker asks share one DM and both frozen bindings; each fake invocation verified the persisted outbox/key before returning IDs")

        self.command("click", "--hit", "settings")
        self.edit("feishu-cli-path", str(self.bin / "not-installed"))
        self.command("wait", "--text", "Feishu notifications (off)")
        self.command("press", "esc")
        self.post("master", agents[0], "[waiting-on-you] Off-period ask")
        self.quiet()
        assert len(self.calls()) == 3
        self.command("click", "--hit", "settings")
        self.edit("feishu-cli-path", "")
        self.command("wait", "--text", "Feishu notifications (on)")
        self.command("press", "esc")
        self.quiet()
        assert len(self.calls()) == 3, "restoring the plugin cannot replay off-period alerts"
        self.step(3, "Missing explicit plugin path disabled Feishu; restoring PATH discovery did not replay the off-period ask")

        (self.bin / "bridge-mode").write_text("fail")
        failed_source = self.post("master", agents[0], "[waiting-on-you] Synthetic failure")
        self.until(lambda: len(self.calls()) == 4, "one failed bridge attempt")
        self.until(lambda: self.state()["settings"]["feishu"]["error"], "failed send status")
        self.quiet()
        assert len(self.calls()) == 4
        failed = next(r for r in self.durable()["feishu"]["outbox"].values() if r["source"]["prompt_id"] == failed_source)
        assert failed["attempted"] and failed["message_id"] is None
        logs = "\n".join(path.read_text(errors="replace") for path in Path(self.session["data_dir"]).rglob("*.log"))
        assert "bus.feishu.failed" in logs and "exit code Some(7)" in logs
        (self.root / "failure-warning.txt").write_text("\n".join(line for line in logs.splitlines() if "bus.feishu.failed" in line))
        assert "feishu-bridge failed" not in str(self.command("snapshot"))
        self.no_popup()
        (self.bin / "bridge-mode").unlink()
        self.step(4, "Exit-7 attempt retained its uncertain outbox record; warning logged, no automatic retry or terminal popup, and scratch Bus stayed responsive")

        self.command("click", "--hit", "settings")
        self.edit("feishu-recipient", "ou_fixture_destination")
        self.edit("feishu-cli-path", str(self.cli))
        self.command("press", "esc")
        configured = self.post("master", agents[0], "[waiting-on-you] Configured recipient")
        self.until(lambda: len(self.calls()) == 5, "configured executable and recipient")
        self.until(lambda: any(r["source"]["prompt_id"] == configured and r["message_id"] is not None
                              for r in self.durable()["feishu"]["outbox"].values()), "configured returned IDs committed")
        assert self.calls()[-1][2] == "ou_fixture_destination"
        self.quiet()
        assert len(self.calls()) == 5, "a later successful request cannot replay the failed attempt"
        self.assert_bridge()
        self.no_popup()
        (self.root / "bridge-calls.json").write_text(json.dumps(self.bridge_records(), indent=2))
        self.step(5, "Explicit bridge path and recipient setting produced one new mapped DM; failed attempt remained spent; complete bridge ledger retained")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--name", default="feishu-outbound")
    args = parser.parse_args()
    if not args.binary.is_absolute() or not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("--binary must be an absolute executable dev-tools Bus")
    run = Run(args.binary, args.name)
    try:
        run.exercise()
    except Exception as error:
        run.fail_remaining(str(error).replace("\n", " "))
        traceback.print_exc()
    finally:
        result = run.finish()
    return result if "Verdict: GREEN" in run.report.read_text() else 1


if __name__ == "__main__":
    raise SystemExit(main())
