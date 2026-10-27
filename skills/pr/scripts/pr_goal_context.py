#!/usr/bin/env python3
"""Run the Bus-owned goal-context writer for the caller's Git repository."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "_bus"))
from cli_extensions.pr_goal_context import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
