from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "sanitize_review_severity.py"


class SanitizeReviewSeverityTests(unittest.TestCase):
    def run_sanitizer(self, source: str) -> tuple[subprocess.CompletedProcess[str], str]:
        with tempfile.TemporaryDirectory() as temp_dir:
            review = Path(temp_dir) / "review.md"
            review.write_text(source, encoding="utf-8", newline="\n")
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--in-place", str(review)],
                cwd=REPO_ROOT,
                text=True,
                capture_output=True,
                check=False,
            )
            return result, review.read_text(encoding="utf-8")

    def test_removes_ranked_heading_and_list_prefixes(self) -> None:
        source = """## Findings

### [Blocker] Startup panic
### [MAJOR] Lost response
### [minor] Misleading copy
### [Critical] Unsafe delete
### P0: First issue
### [P1] Second issue
- (P2) Third issue
- P3 - Fourth issue
"""

        result, sanitized = self.run_sanitizer(source)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            sanitized,
            """## Findings

### Startup panic
### Lost response
### Misleading copy
### Unsafe delete
### First issue
### Second issue
- Third issue
- Fourth issue
""",
        )
        self.assertIn("sanitized 8 severity tags", result.stdout)

    def test_removes_severity_metadata_lines_and_compound_prefixes(self) -> None:
        source = """## Findings

### [High severity] [P1] Broken routing
- **Severity:** High
- Priority: medium
Severity level — low
Impact: all selected agents miss the message.
"""

        result, sanitized = self.run_sanitizer(source)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            sanitized,
            """## Findings

### Broken routing
Impact: all selected agents miss the message.
""",
        )
        self.assertIn("sanitized 5 severity tags", result.stdout)

    def test_removes_bold_wrapped_severity_prefixes(self) -> None:
        source = """## Findings

### **[Major]** Lost response
- **P2:** Stale status
"""

        result, sanitized = self.run_sanitizer(source)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            sanitized,
            """## Findings

### Lost response
- Stale status
""",
        )
        self.assertIn("sanitized 2 severity tags", result.stdout)

    def test_preserves_non_label_language_and_source_identifiers(self) -> None:
        source = """## Findings

### F-01 Major version parsing is incorrect
- Evidence: `Priority::P1` and `parse_p0()` are source identifiers.
- Impact: the critical path retries forever.
"""

        result, sanitized = self.run_sanitizer(source)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sanitized, source)
        self.assertIn("sanitized 0 severity tags", result.stdout)

    def test_check_mode_reports_dirty_input_without_modifying_it(self) -> None:
        source = "### [Major] Retry loop\n"
        with tempfile.TemporaryDirectory() as temp_dir:
            review = Path(temp_dir) / "review.md"
            review.write_text(source, encoding="utf-8", newline="\n")

            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--check", str(review)],
                cwd=REPO_ROOT,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(result.returncode, 1)
            self.assertEqual(review.read_text(encoding="utf-8"), source)
            self.assertIn("contains 1 severity tag", result.stderr)


if __name__ == "__main__":
    unittest.main()
