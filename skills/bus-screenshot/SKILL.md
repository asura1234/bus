---
name: bus-screenshot
description: Take a screenshot of the running Bus UI (the iTerm2 or Ghostty window that runs the Bus client), save it as temp/screenshots/bus-<timestamp>.png and report the path, so UI changes can be checked without the human taking screenshots by hand. macOS only. Use when asked to "screenshot Bus", "show me the Bus UI", "check how it looks", or to verify a TUI change in the live Bus.
---

# bus-screenshot

Run from the Bus repository root:

```sh
python3 skills/bus-screenshot/scripts/bus_screenshot.py [--window ID | --title TEXT] [--out-dir DIR]
```

- It finds the iTerm2 or Ghostty window whose title shows it runs `bus` (the shell
  titles it `host: bus`; a shell that only sits in the repo, `~/work/bus`, does not
  count). With several, it takes the frontmost window on screen. `--title TEXT` matches
  any title containing TEXT instead; `--window ID` takes a CoreGraphics window id.
- It captures only that window, silently and without focusing it, to
  `temp/screenshots/bus-YYYYmmdd-HHMMSS.png` (or `--out-dir`).
- The last stdout line is the PNG's absolute path. Open it with your image reader to
  check the UI, and give the path when you report.
- On failure it exits 1 with the reason on stderr: no Bus window, the window is not on
  screen (minimized or on another Space), or the terminal app that runs the command
  lacks Screen Recording permission. The message names the app to allow in System
  Settings > Privacy & Security > Screen Recording; the app must be restarted after.

Screenshots stay under `temp/`, which git ignores.
