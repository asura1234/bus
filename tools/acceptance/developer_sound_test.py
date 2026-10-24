#!/usr/bin/env python3
"""Opt-in scratch TUI acceptance for the developer sound checkbox and selection."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import traceback

if __package__:
    from . import feishu_notify_test as helper
else:
    import feishu_notify_test as helper

helper.DOC = "tests/e2e-tui/developer-sound.md"


class Run(helper.Run):
    def sounds(self):
        return self.command("effects", "--kind", "sound").get("effects", [])

    def preference(self):
        return self.state()["settings"]["developer_sound"]

    def choice(self):
        return self.preference()["name"]

    def saved(self):
        return json.loads((Path(self.session["data_dir"]) / "settings.json").read_text())

    def sound_path(self, name):
        for directory in (Path("/System/Library/Sounds"), Path("/Library/Sounds")):
            for path in directory.glob("*"):
                if path.stem.casefold() == name.casefold() and path.is_file():
                    return str(path)
        raise AssertionError(f"No installed system sound named {name}")

    def change(self, verb, *args):
        before, count = self.preference(), len(self.sounds())
        self.command(verb, *args)
        self.until(lambda: self.choice() != before["name"], "sound choice saved")
        choice = self.choice()
        self.until(lambda: len(self.sounds()) == count + 1, "one recorded preview")
        assert self.sounds()[-1]["target"] == self.sound_path(choice or "Glass")
        assert self.preference()["enabled"] == before["enabled"]
        return choice

    def select(self, name):
        for _ in range(100):
            if self.choice() == name:
                return
            self.change("click", "--hit", "developer-sound:next")
        raise AssertionError(f"Sound choice {name} not reachable")

    def toggle(self, verb, *args):
        before, count = self.preference(), len(self.sounds())
        self.command(verb, *args)
        self.until(lambda: self.preference()["enabled"] != before["enabled"], "checkbox saved")
        assert self.choice() == before["name"]
        assert len(self.sounds()) == count
        saved = self.saved()
        assert saved["developer_sound_enabled"] == self.preference()["enabled"]
        assert saved["developer_sound"] == before["name"]
        assert saved["feishu_waiting_notifications"]

    def post(self, text, developer=False):
        args = ["send", "--room", "sound-work", "--to", "sound-worker" if developer else "you", "--text", text]
        if not developer:
            args += ["--as", "sound-worker"]
        return self.bus(*args)

    def open_settings(self):
        self.command("click", "--hit", "settings")
        self.command("wait", "--text", r"\[[x ]\] You", "--regex")

    def case_section(self):
        body = self.report.read_text()
        start = body.index(f"## {self.case} —")
        end = body.find("\n## ", start + 1)
        return body, start, len(body) if end < 0 else end

    def start_case(self, case, size="160x55"):
        self.case = case
        self.name = f"you-{self.base_name[-8:]}-{case[-2:]}"
        self.session = self.command("start", "--size", size, "--graphics", "none", "--run-dir", self.root / "sessions")
        body, start, end = self.case_section()
        section = body[start:end].replace("- Session: `none`", f"- Session: `{self.session['run_dir']}`")
        self.report.write_text(body[:start] + section + body[end:])

    def step(self, number, evidence):
        body, start, end = self.case_section()
        section = body[start:end]
        first = section.index(f"### Step {number}\n")
        last = section.find("\n### Step ", first + 1)
        if last < 0:
            last = len(section)
        block = section[first:last]
        assert "- Result: `PENDING`" in block
        block = block.replace("`PENDING`", "`PASS`").replace("- Evidence: none yet", f"- Evidence: {evidence}; numbered command JSON, cells and trace under `{self.root}`.")
        section = section[:first] + block + section[last:]
        self.report.write_text(body[:start] + section + body[end:])
        print(f"{self.case} step {number}: PASS", flush=True)

    def capture(self, name):
        self.command("snapshot")
        columns, rows = self.command("status")["size"]
        self.command("cells", "0", "0", columns, rows)
        self.command("screenshot", "--out", self.root / f"{name}.png")

    def selected_you(self):
        found = self.command("find", r"\[[x ]\] You", "--regex")["matches"]
        assert len(found) == 1
        match = found[0]
        cells = self.command("cells", match["row"], match["col"], match["width"], 1)["cells"]
        assert all(cell["bg"] == "#2e303a" for cell in cells)

    def exercise_sound(self):
        for name in ("sound-work", "sound-other"):
            self.bus("room", "create", name)
        work = Path(self.session["data_dir"]).parent / "work"
        self.bus("agent", "add", "--room", "sound-work", "--name", "sound-worker", "--provider", "claude", "--pwd", work, "--consent-hooks")
        self.command("wait", "--state", "agents.*.status == idle", "--timeout", "20s")
        self.bus("room", "sound", "master", "--off")
        self.open_settings()
        def row(text):
            return next(match["row"] for match in self.command("find", text)["matches"] if match["col"] >= 30)
        assert row("Sound notifications") < row("YOU") < row("[x] You") < row("MASTER") < row("ROOMS")
        assert row("NOTIFICATIONS") < row("Feishu notifications (always on)") < row("Recipient open_id") < row("lark-cli path")
        assert self.preference() == {"enabled": True, "name": None}
        assert all(not room["sound"] for room in self.state()["rooms"])
        self.capture("system-default")
        self.step(1, "Checked You/System default is first under YOU above MASTER/ROOMS; Feishu/recipient/CLI remain under NOTIFICATIONS; all rooms muted; system-default.png captured")

        first = self.change("click", "--hit", "developer-sound:next")
        assert first is not None
        self.change("press", "right")
        assert self.change("press", "left") == first
        self.select("Glass")
        assert self.saved()["developer_sound"] == "Glass" and self.saved()["developer_sound_enabled"]
        assert self.preference() == {"enabled": True, "name": "Glass"}
        self.capture("glass-selected")
        self.step(2, "Every mouse/key choice previewed its installed file; Glass and enabled=true persisted with matching state object; glass-selected.png captured")

        self.command("press", "esc")
        self.command("click", "--hit", "room:sound-other")
        self.command("type", "KEEP DRAFT")
        self.command("focus", "out")
        count = len(self.sounds())
        self.post("[waiting-on-you] Choose release timing")
        self.until(lambda: len(self.sounds()) == count + 1, "one Glass developer alert")
        assert self.sounds()[-1]["target"] == self.sound_path("Glass")
        self.quiet()
        self.command("snapshot")
        self.quiet()
        assert len(self.sounds()) == count + 1
        self.command("wait", "--text", "KEEP DRAFT")
        assert next(room["name"] for room in self.state()["rooms"] if room["id"] == self.state()["visible_room"]) == "sound-other"
        self.no_popup()
        self.step(3, "Hidden-room notice played exactly one Glass.aiff with muted rooms/focus out; draft and visible room retained, no popup or replay")

        self.open_settings()
        self.command("press", "down")
        self.toggle("press", "enter")
        self.command("wait", "--text", "[ ] You")
        self.selected_you()
        assert self.preference() == {"enabled": False, "name": "Glass"}
        self.capture("you-muted")
        (self.root / "muted-settings.json").write_text(json.dumps(self.saved(), indent=2))
        self.command("press", "esc")
        count, calls = len(self.sounds()), len(self.sends())
        self.post("[waiting-on-you] Muted audio still sends Feishu")
        self.quiet()
        assert len(self.sounds()) == count
        assert any("Muted audio still sends Feishu" in args[7] for args in self.sends()[calls:])
        assert self.state()["settings"]["feishu_waiting_notifications"]
        self.step(4, "Enter unchecked You, retained Glass and persisted enabled=false; qualifying notice made no sound but recording lark-cli captured its Feishu DM; you-muted.png and muted-settings.json retained")

        self.open_settings()
        self.command("press", "down")
        self.toggle("press", "enter")
        self.select("Ping")
        self.capture("ping-selected")
        self.command("press", "esc")
        count = len(self.sounds())
        self.post("[waiting-on-you] Restored Ping alert")
        self.until(lambda: len(self.sounds()) == count + 1, "one restored Ping alert")
        assert self.sounds()[-1]["target"] == self.sound_path("Ping")
        self.quiet()
        assert len(self.sounds()) == count + 1
        assert self.preference() == {"enabled": True, "name": "Ping"}
        assert self.saved()["developer_sound"] == "Ping" and self.saved()["developer_sound_enabled"]
        assert all(not room["sound"] for room in self.state()["rooms"])
        count = len(self.sounds())
        sent = self.post("[waiting-on-you] Developer own send fake:raw-reply done", developer=True)
        self.command("wait", "--message", sent["result"]["message_id"], "--timeout", "20s")
        self.quiet()
        assert len(self.sounds()) == count
        sounds = self.sounds()
        assert all(Path(effect["target"]).is_file() and "bus-developer-alert" not in effect["target"] for effect in sounds)
        (self.root / "sound-effects.json").write_text(json.dumps(sounds, indent=2))
        self.step(5, "Enter restored enabled audio; Ping persisted and rang exactly once; muted room switches unchanged; developer-authored waiting prompt stayed silent; all effects installed system files; ping-selected.png retained")

    def exercise_scroll(self):
        self.bus("room", "rename", "1", "scroll-00")
        for index in range(1, 12):
            self.bus("room", "create", f"scroll-{index:02d}")
        self.bus("room", "sound", "master", "--off")
        self.open_settings()
        self.command("press", "down")
        self.selected_you()
        self.capture("scroll-focus")
        self.step(1, "One Down selects checked You below YOU above MASTER/ROOMS; selected cells use #2e303a; twelve room fixtures; scroll-focus.png captured")

        self.command("resize", "160x24")
        self.toggle("click", "--hit", "sound:you")
        self.selected_you()
        self.command("press", "space")
        assert not self.preference()["enabled"]
        first = self.change("click", "--hit", "developer-sound:next")
        self.change("press", "right")
        assert self.change("press", "left") == first
        self.selected_you()
        assert not self.preference()["enabled"]
        self.toggle("click", "--hit", "sound:you")
        self.command("press", "down")
        self.command("press", "enter")
        assert all(room["sound"] == (room["kind"] == "master") for room in self.state()["rooms"])
        assert self.choice() == first
        self.step(2, "At 160x24 mouse checkbox mutes/restores You, Space inert; next/Right/Left preview and save while unchecked without enabling it; Down+Enter toggles only MASTER")

        self.command("scroll", "--text", "[x] You", "--down", "30")
        for text in ("NOTIFICATIONS", "Feishu notifications (always on)", "Recipient open_id", "lark-cli path"):
            assert self.command("find", text)["count"]
        for _ in range(22):
            self.command("press", "down")
        match = self.command("find", "Performance overlay")["matches"][0]
        before = self.command("cells", match["row"], match["col"], 19, 1)["cells"]
        assert all(cell["bg"] == "#2e303a" for cell in before)
        self.command("press", "down")
        assert self.command("cells", match["row"], match["col"], 19, 1)["cells"] == before
        self.capture("scrolled")
        self.step(3, "Wheel reveals Feishu group intact; Down reaches visible selected final Performance overlay and clamps there; heading and row fit above footer; scrolled.png captured")

        for _ in range(24):
            self.command("press", "up")
        self.command("press", "down")
        assert self.command("find", "YOU")["matches"][0]["row"] == 10
        self.selected_you()
        self.change("click", "--hit", "developer-sound:previous")
        assert self.preference()["enabled"]
        self.capture("restored")
        (self.root / "scroll-effects.json").write_text(json.dumps(self.sounds(), indent=2))
        self.step(4, "Up then Down restores YOU and selected checked You; previous arrow previews/persists System default without changing checkbox; restored.png and effects retained")

    def fail_case(self, reason):
        body, start, end = self.case_section()
        section = body[start:end]
        screenshot, trace = "none", "none"
        if self.session:
            trace = str(Path(self.session["run_dir"]) / "trace.ndjson")
            try:
                screenshot = str(self.root / f"{self.case}-failed.png")
                self.command("screenshot", "--out", screenshot)
            except Exception:
                screenshot = "none"
        blocks = section.split("### Step ")
        for index in range(1, len(blocks)):
            if "`PENDING`" in blocks[index]:
                number = blocks[index].split("\n", 1)[0]
                blocks[index] = f'''{number}

- Result: `FAIL`
- Evidence: Acceptance stopped: {reason}
- Expected: Complete the documented step and assertions
- Seen: {reason}
- Trace: `{trace}`
- Screenshot: `{screenshot}`
- Effects: Numbered driver calls and recorded-effects ledger retained
- Fault guess: `product`
- Fault evidence: An assertion or command failed; inspect the captured invocation and trace

'''
        self.report.write_text(body[:start] + "### Step ".join(blocks) + body[end:])

    def finish(self):
        checked = subprocess.run([sys.executable, str(helper.REPORT_TOOL), "check", "--doc", helper.DOC, str(self.report)], cwd=helper.ROOT, capture_output=True, text=True)
        self.report.write_text(self.report.read_text() + "\n## Summary\n\n" + checked.stdout + checked.stderr)
        print(f"{checked.stdout.strip()} — {self.report}", flush=True)
        return checked.returncode if "Verdict: GREEN" in checked.stdout else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--run-name", required=True)
    args = parser.parse_args()
    run = Run(args.binary, args.run_name)
    run.base_name = run.name
    for case, exercise in (("DSOUND-01", run.exercise_sound), ("DSOUND-02", run.exercise_scroll)):
        run.case, run.session = case, None
        try:
            run.start_case(case)
            exercise()
        except Exception as error:
            run.fail_case(str(error).replace("\n", " "))
            traceback.print_exc()
        finally:
            run.command("stop", ok=False)
    return run.finish()


if __name__ == "__main__":
    raise SystemExit(main())
