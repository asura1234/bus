"""The outcome_log example format in --help must be recognized by seeds()."""

from __future__ import annotations

import sys
from pathlib import Path


SCRIPTS_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS_DIR))

from best_of_n import seeds  # noqa: E402
from test_best_of_n import ledger  # noqa: E402


def test_help_schema_outcome_log_is_excluded_from_remaining_seeds() -> None:
    """The schema example in `--help` is `2026-09-16 A failed: <observation>, switching to B`.

    SKILL.md points the schema at this --help, and switching relies on seeds() extracting attempted
    options from outcome_log. If the example format is not recognized, the already failed A becomes a
    seed again after B also fails.
    """

    data = ledger()
    data["disputes"][0]["outcome_log"] = ["2026-09-16 A failed: still reproduces after the scrub cancel change, switching to B"]

    out = seeds(data, "C3", "B")

    assert "EXHAUSTED" in out
    assert "the transport owner releases it uniformly" not in out
