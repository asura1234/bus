#!/usr/bin/env python3
"""Opt-in mandatory-developer-alert acceptance; importing launches nothing."""
import argparse
import json
from pathlib import Path
import traceback

if __package__:
    from . import feishu_notify_test as helper
else:
    import feishu_notify_test as helper

helper.DOC = "tests/e2e-tui/notify-you.md"


class Run(helper.Run):
    def post_room(self, room, sender, recipient, text, *extra, ok=True):
        args = ["send", "--room", room, "--to", recipient, "--text", text]
        if sender:
            args += ["--as", sender]
        return self.bus(*args, *extra, ok=ok)

    def sounds(self):
        return self.command("effects", "--kind", "sound").get("effects", [])

    def mandatory(self):
        return [effect for effect in self.sounds() if Path(effect["target"]).name == "Glass.aiff"]

    def delivered(self, text):
        self.until(lambda: any(text in call[7] for call in self.calls()), text)

    def exercise(self):
        self.session = self.command("start", "--size", "160x55", "--graphics", "none", "--run-dir", self.root / "sessions")
        self.report.write_text(self.report.read_text().replace("- Session: `none`", f"- Session: `{self.session['run_dir']}`"))
        for room in ["notify-work", "notify-other"]:
            self.bus("room", "create", room)
        work = Path(self.session["data_dir"]).parent / "work"
        self.bus("agent", "add", "--room", "master", "--name", "notify-orch", "--provider", "claude", "--pwd", work, "--orchestrates", "notify-work", "--consent-hooks")
        self.bus("agent", "add", "--room", "notify-work", "--name", "notify-worker", "--provider", "claude", "--pwd", work, "--consent-hooks")
        self.until(lambda: all(agent["status"] == "idle" for agent in self.state()["agents"]), "all fake agents idle")
        for room in ["master", "notify-work", "notify-other"]:
            self.bus("room", "sound", room, "--off")
        assert all(not room["sound"] for room in self.state()["rooms"]), "room sound fixture did not mute every room"
        settings_path = Path(self.session["data_dir"]) / "settings.json"
        saved = json.loads(settings_path.read_text())
        saved["feishu_waiting_notifications"] = False
        settings_path.write_text(json.dumps(saved))
        self.command("press", "f6")
        self.command("click", "--hit", "settings")
        self.command("wait", "--text", "Feishu notifications (always on)")
        screen = str(self.command("snapshot"))
        assert "Recipient open_id:" in screen and "lark-cli path:" in screen, screen
        assert self.state()["settings"]["feishu_waiting_notifications"] is True
        self.command("click", "--hit", "feishu-notifications")
        self.command("press", "enter")
        self.command("wait", "--text", "Feishu notifications (always on)")
        self.capture("settings-mandatory")
        self.command("press", "esc")
        self.command("click", "--hit", "room:notify-other")
        self.command("type", "KEEP DRAFT")
        self.quiet()
        sounds, calls = len(self.mandatory()), len(self.calls())
        self.step(1, "Always-on Feishu row stayed on after Enter; legacy false read as true; recipient and CLI path visible, room sounds disabled; KEEP DRAFT entered; settings-mandatory.png captured")

        before_history = self.bus("history", "--room", "notify-work")["result"]
        rejected = self.post_room("notify-work", "notify-worker", "you", "Decide release timing", ok=False)
        assert rejected["ok"] is False, rejected
        error = rejected["error"]["message"]
        assert all(part in error for part in ["--to orchestrator", "[waiting-on-you]", "developer must do"]), error
        assert self.bus("history", "--room", "notify-work")["result"] == before_history
        self.quiet()
        assert len(self.mandatory()) == sounds and len(self.calls()) == calls
        sent = self.post_room("notify-work", "notify-worker", "you", "[waiting-on-you] Decide release timing")
        assert sent["result"]["stage"] == "posted" and sent["result"]["request_ids"] == []
        history = self.bus("history", "--room", "notify-work")
        (self.root / "worker-history.json").write_text(json.dumps(history, indent=2))
        sender_id = next(agent["id"] for agent in self.state()["agents"] if agent["name"] == "notify-worker")
        posted = next(message["prompt"] for message in history["result"]["messages"] if message["prompt"]["text"] == "[waiting-on-you] Decide release timing")
        assert posted["author"] == {"agent": sender_id} and posted["recipient_ids"] == [], posted
        self.until(lambda: len(self.mandatory()) == sounds + 1, "worker mandatory sound")
        self.delivered("notify-worker is waiting on you: Decide release timing")
        assert len(self.calls()) == calls + 1
        self.command("wait", "--text", "KEEP DRAFT")
        assert self.state()["visible_room"] == next(room["id"] for room in self.state()["rooms"] if room["name"] == "notify-other")
        self.quiet()
        assert len(self.mandatory()) == sounds + 1 and len(self.calls()) == calls + 1
        self.no_popup()
        self.step(2, "Untagged worker --to you rejected with actionable guidance and unchanged history/alerts; tagged send posted without requests in notify-work history; one distinct recorded mandatory sound and exact bot DM; hidden-room draft/focus unchanged and no replay or popup")

        self.command("click", "--hit", "room:notify-work")
        self.command("wait", "--text", "Decide release timing")
        assert "You" in str(self.command("snapshot"))
        sounds = len(self.mandatory())
        self.post_room("notify-work", "notify-orch", "you", "[waiting-on-you] Approve access")
        self.post_room("master", "notify-worker", "you", "[waiting_on_you] MASTER decision")
        self.until(lambda: len(self.mandatory()) == sounds + 2, "two mandatory burst sounds")
        self.delivered("notify-orch is waiting on you: Approve access")
        self.delivered("notify-worker is waiting on you: MASTER decision")
        for room, text in [("notify-work", "Approve access"), ("master", "MASTER decision")]:
            assert text in str(self.bus("history", "--room", room))
        self.capture("worker-to-you")
        self.step(3, "Visible history shows You; orchestrator work-room blocker and worker MASTER post kept their rooms and authors; two mandatory effects bypassed ordinary cooldown and both actions reached bot recorder; worker-to-you.png captured")

        self.command("click", "--hit", "room:notify-other")
        self.command("focus", "out")
        sounds = len(self.mandatory())
        tagged = self.post_room("notify-work", "notify-worker", "notify-orch", "[waiting-on-you] Choose rollout")
        self.command("wait", "--message", tagged["result"]["message_id"], "--timeout", "20s")
        self.until(lambda: len(self.mandatory()) > sounds, "tagged peer mandatory sound")
        self.delivered("notify-worker is waiting on you: Choose rollout")
        before_history = self.bus("history", "--room", "notify-work")["result"]
        self.quiet()
        mention_sounds, mention_calls = len(self.mandatory()), len(self.calls())
        rejected = self.post_room("notify-work", "notify-worker", "notify-orch", "@you Please choose rollout", ok=False)
        assert rejected["ok"] is False and "--to orchestrator" in rejected["error"]["message"], rejected
        assert self.bus("history", "--room", "notify-work")["result"] == before_history
        self.quiet()
        assert len(self.mandatory()) == mention_sounds and len(self.calls()) == mention_calls
        mentioned = self.post_room("notify-work", "notify-worker", "notify-orch", "@you [waiting-on-you] Please choose rollout")
        self.command("wait", "--message", mentioned["result"]["message_id"], "--timeout", "20s")
        self.delivered("notify-worker is waiting on you: Please choose rollout")
        self.quiet()
        sounds, calls = len(self.mandatory()), len(self.calls())
        for sender, text in [("notify-worker", "PR is pushed; still waiting on you for the next task"), (None, "[waiting-on-you] Developer send fake:raw-reply done")]:
            sent = self.post_room("notify-work", sender, "notify-orch", text)
            self.command("wait", "--message", sent["result"]["message_id"], "--timeout", "20s")
        self.quiet()
        assert len(self.mandatory()) == sounds and len(self.calls()) == calls
        self.command("wait", "--text", "KEEP DRAFT")
        self.step(4, "Untagged @you rejected without history/request/alert; tagged peer send and tagged @you notified with focus out and muted rooms; untagged peer waiting phrase and developer-authored tagged prompt produced no mandatory sound or bot call; KEEP DRAFT preserved")

        self.post_room("notify-other", "notify-worker", "human", "[waiting_on_you] Alias delivery")
        self.delivered("notify-worker is waiting on you: Alias delivery")
        self.quiet()
        sounds, calls = len(self.mandatory()), len(self.calls())
        for sender, recipient, extra in [("notify-worker", "you,notify-orch", ()), ("notify-worker", "you", ("--queue",)), ("notify-worker", "you", ("--async",)), (None, "you", ())]:
            response = self.post_room("notify-other", sender, recipient, "[waiting-on-you] Rejected message", *extra, ok=False)
            assert response.get("ok") is False, response
        assert self.bus("settings", "feishu-notifications", "--off", ok=False)["ok"] is False
        self.quiet()
        assert len(self.mandatory()) == sounds and len(self.calls()) == calls
        self.bus("room", "sound", "notify-work", "--on")
        sent = self.post_room("notify-work", "notify-worker", "notify-orch", "Ordinary room sound")
        self.command("wait", "--message", sent["result"]["message_id"], "--timeout", "20s")
        self.until(lambda: any("bus-sound-" in str(effect) for effect in self.sounds()), "ordinary room sound recorder")
        self.assert_bot()
        self.no_popup()
        (self.root / "calls.json").write_text(json.dumps(self.calls(), indent=2))
        (self.root / "sound-effects.json").write_text(json.dumps(self.sounds(), indent=2))
        self.step(5, "human alias notified from third room; mixed recipients, queue, async, missing author and Feishu off rejected without new alerts; ordinary bus-sound target differs from mandatory system Glass.aiff target; bot argv and no terminal popup verified")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--run-name", required=True)
    args = parser.parse_args()
    run = Run(args.binary, args.run_name)
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
