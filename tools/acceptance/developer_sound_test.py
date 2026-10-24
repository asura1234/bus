#!/usr/bin/env python3
"""Opt-in scratch TUI acceptance for developer system-sound selection."""
import argparse
import json
from pathlib import Path
import traceback

if __package__:
    from . import feishu_notify_test as helper
else:
    import feishu_notify_test as helper

helper.DOC = "tests/e2e-tui/developer-sound.md"


class Run(helper.Run):
    def sounds(self):
        return self.command("effects", "--kind", "sound").get("effects", [])

    def choice(self):
        return self.state()["settings"]["developer_sound"]

    def sound_path(self, name):
        for directory in (Path("/System/Library/Sounds"), Path("/Library/Sounds")):
            for path in directory.glob("*"):
                if path.stem.casefold() == name.casefold() and path.is_file():
                    return str(path)
        raise AssertionError(f"No installed system sound named {name}")

    def change(self, verb, *args):
        before, count = self.choice(), len(self.sounds())
        self.command(verb, *args)
        self.until(lambda: self.choice() != before, "sound choice saved")
        choice = self.choice()
        expected = self.sound_path(choice or "Glass")
        self.until(lambda: len(self.sounds()) == count + 1, "one recorded preview")
        assert self.sounds()[-1]["target"] == expected
        return choice

    def select(self, name):
        for _ in range(100):
            if self.choice() == name:
                return
            self.change("click", "--hit", "developer-sound:next")
        raise AssertionError(f"Sound choice {name} not reachable")

    def post(self, text, developer=False):
        args = ["send", "--room", "sound-work", "--to", "sound-worker" if developer else "you", "--text", text]
        if not developer:
            args += ["--as", "sound-worker"]
        return self.bus(*args)

    def open_settings(self):
        self.command("click", "--hit", "settings")
        self.command("wait", "--text", "Notify you sound (always on)")

    def exercise(self):
        self.session = self.command("start", "--size", "160x55", "--graphics", "none", "--run-dir", self.root / "sessions")
        self.report.write_text(self.report.read_text().replace("- Session: `none`", f"- Session: `{self.session['run_dir']}`"))
        for name in ("sound-work", "sound-other"):
            self.bus("room", "create", name)
        work = Path(self.session["data_dir"]).parent / "work"
        self.bus("agent", "add", "--room", "sound-work", "--name", "sound-worker", "--provider", "claude", "--pwd", work, "--consent-hooks")
        self.until(lambda: all(agent["status"] == "idle" for agent in self.state()["agents"]), "fake worker idle")
        for name in ("master", "sound-work", "sound-other"):
            self.bus("room", "sound", name, "--off")
        self.command("press", "f6")
        self.open_settings()
        screen = str(self.command("snapshot"))
        assert "System default" in screen and "Feishu notifications (always on)" in screen
        assert self.choice() is None
        assert all(not room["sound"] for room in self.state()["rooms"])
        self.capture("system-default")
        self.step(1, "Notify you sound (always on) shows System default; Feishu always on and all room sounds off; system-default.png captured")

        first = self.change("click", "--hit", "developer-sound:next")
        assert first is not None
        self.change("press", "right")
        assert self.change("press", "left") == first
        self.select("Glass")
        self.capture("glass-selected")
        saved = json.loads((Path(self.session["data_dir"]) / "settings.json").read_text())
        assert saved["developer_sound"] == "Glass"
        self.step(2, "Mouse and keyboard cycle system sounds, every preview targets its installed file; Glass saved in developer_sound; glass-selected.png captured")

        self.command("press", "esc")
        self.command("click", "--hit", "room:sound-other")
        self.command("type", "KEEP DRAFT")
        self.command("focus", "out")
        self.quiet()
        count = len(self.sounds())
        sent = self.post("[waiting-on-you] Choose release timing")
        assert sent["result"]["request_ids"] == []
        self.until(lambda: len(self.sounds()) == count + 1, "one Glass developer alert")
        assert self.sounds()[-1]["target"] == self.sound_path("Glass")
        self.quiet()
        assert len(self.sounds()) == count + 1
        self.command("wait", "--text", "KEEP DRAFT")
        assert next(room["name"] for room in self.state()["rooms"] if room["id"] == self.state()["visible_room"]) == "sound-other"
        self.no_popup()
        self.step(3, "Hidden work-room developer notice recorded one Glass.aiff playback with room sounds off and focus out; KEEP DRAFT and visible room preserved; no replay or terminal popup")

        self.open_settings()
        self.change("click", "--hit", "developer-sound:next")
        self.change("press", "enter")
        self.select("Ping")
        assert "always on" in str(self.command("snapshot"))
        self.capture("ping-selected")
        self.command("press", "esc")
        self.quiet()
        count = len(self.sounds())
        self.post("[waiting-on-you] Approve release")
        self.until(lambda: len(self.sounds()) == count + 1, "one Ping developer alert")
        assert self.sounds()[-1]["target"] == self.sound_path("Ping")
        assert all(not room["sound"] for room in self.state()["rooms"])
        self.step(4, "Enter changes the sound without disabling alerts; Ping saved and one tagged alert recorded Ping.aiff while all room sounds remain off; ping-selected.png captured")

        saved = json.loads((Path(self.session["data_dir"]) / "settings.json").read_text())
        assert saved["developer_sound"] == "Ping" and saved["feishu_waiting_notifications"]
        self.quiet()
        count = len(self.sounds())
        sent = self.post("[waiting-on-you] Developer own send fake:raw-reply done", developer=True)
        self.command("wait", "--message", sent["result"]["message_id"], "--timeout", "20s")
        self.quiet()
        assert len(self.sounds()) == count
        sounds = self.sounds()
        assert all(Path(effect["target"]).is_file() and str(effect["target"]).startswith(("/System/Library/Sounds/", "/Library/Sounds/")) for effect in sounds)
        assert all("bus-developer-alert" not in effect["target"] for effect in sounds)
        (self.root / "sound-effects.json").write_text(json.dumps(sounds, indent=2))
        self.step(5, "Saved Ping and Feishu-on preferences confirmed; developer-authored waiting prompt made no sound; every preview/alert effect targets an installed system file, with no bundled developer MP3 target; sound-effects.json retained")


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
