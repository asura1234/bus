from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import e2e_report  # noqa: E402

DOC = """# QA

## Run contract

1. Not a case step.

## Cases

### APP-01 — First start

1. Launch → it starts.
2. Type → it types.

**Evidence:** text.

### APP-02 — Rooms

1. Create → a room.

## Decisions

1. Not a step either.
"""


def filled(results: dict[str, list[str]]) -> str:
    cases = e2e_report.select(e2e_report.parse_doc(DOC), ",".join(results))
    text = e2e_report.render_skeleton("doc.md", cases, "abc", "20261012-000000", "macOS")
    for case_id, steps in results.items():
        for result in steps:
            block = f"- Result: `{result}`\n- Evidence: seen it"
            if result == "FAIL":
                block += (
                    "\n- Expected: x\n- Seen: y\n- Trace: `t`\n- Screenshot: none"
                    "\n- Effects: none\n- Fault guess: `product`\n- Fault evidence: z"
                )
            if result == "SKIP":
                block += "\n- Reason: no clipboard"
            text = text.replace("- Result: `PENDING`\n- Evidence: none yet", block, 1)
    return text


class E2eReportTest(unittest.TestCase):
    def setUp(self) -> None:
        self.cases = e2e_report.parse_doc(DOC)

    def test_cases_and_their_numbered_steps_come_from_the_doc(self) -> None:
        self.assertEqual([(c.id, c.title, c.steps) for c in self.cases],
                         [("APP-01", "First start", [1, 2]), ("APP-02", "Rooms", [1])])

    def test_a_filled_report_gives_case_results_and_the_verdict(self) -> None:
        errors, summary = e2e_report.check(
            filled({"APP-01": ["PASS", "SKIP"], "APP-02": ["PASS"]}), self.cases
        )
        self.assertEqual(errors, [])
        self.assertEqual(summary, ["APP-01 PARTIAL", "APP-02 PASS"])
        errors, summary = e2e_report.check(filled({"APP-02": ["FAIL"]}), self.cases)
        self.assertEqual((errors, summary), ([], ["APP-02 FAIL"]))
        summarised = filled({"APP-02": ["FAIL"]}) + "\n## Summary\n\n```text\nAPP-02 FAIL\n```\n"
        self.assertEqual(e2e_report.check(summarised, self.cases), ([], ["APP-02 FAIL"]))

    def test_incomplete_or_malformed_reports_are_refused(self) -> None:
        cases = e2e_report.select(self.cases, "APP-02")
        pending = e2e_report.render_skeleton("doc.md", cases, "abc", "t", "p")
        self.assertIn("APP-02 step 1: still PENDING", e2e_report.check(pending, self.cases)[0])
        bare_fail = pending.replace("`PENDING`\n- Evidence: none yet", "`FAIL`\n- Evidence: e")
        self.assertTrue(any("FAIL lacks" in e for e in e2e_report.check(bare_fail, self.cases)[0]))
        bare_skip = pending.replace("`PENDING`\n- Evidence: none yet", "`SKIP`\n- Evidence: e")
        self.assertIn("APP-02 step 1: SKIP needs a Reason", e2e_report.check(bare_skip, self.cases)[0])
        dropped = filled({"APP-01": ["PASS", "PASS"]}).replace("### Step 2", "### Step 3")
        self.assertTrue(e2e_report.check(dropped, self.cases)[0])
        with self.assertRaises(ValueError):
            e2e_report.select(self.cases, "APP-09")

    def test_init_and_check_on_disk(self) -> None:
        with tempfile.TemporaryDirectory() as root:
            doc = Path(root, "doc.md")
            doc.write_text(DOC, encoding="utf-8")
            self.assertEqual(
                e2e_report.main(["init", "--doc", str(doc), "--cases", "APP-02", "--commit", "abc",
                                 "--platform", "macOS", "--runs-root", root]),
                0,
            )
            report = next(Path(root).glob("*/report.md"))
            self.assertEqual(e2e_report.main(["check", "--doc", str(doc), str(report)]), 1)
            report.write_text(filled({"APP-02": ["PASS"]}), encoding="utf-8")
            self.assertEqual(e2e_report.main(["check", "--doc", str(doc), str(report)]), 0)


if __name__ == "__main__":
    unittest.main()
