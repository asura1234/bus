#!/usr/bin/env python3
"""Structural validation, deterministic rendering, and switch queries for Best-of-N adjudication records.

Judgment belongs to main; this script only blocks the "looks done, but no decision was actually made"
shapes: tied ranks, silently dropped options, disqualifications without reasons, a missing failure
trigger. The most important of these is that **options must not be lost** — the whole point of the
ranking is an examined fallback when the first place fails, and an option deleted at adjudication
time is one nobody will discover ever existed, weeks later.

ledger schema (JSON):

  {
    "goal": "one-sentence locked goal",
    "sources": ["claude", "codex", "cursor"],   # every participant in this round's fan-out
    "adjudicator": "codex",                     # who adjudicates; the orchestrator has too little context, so the adjudicator is necessarily a participant
    "deferred_sources": ["cursor"],             # optional: participants whose only opinionated dispute was handed back
                                                # to the orchestrator and who therefore have no option in this record.
                                                # They stay in sources — that is the denominator of universal and must not shrink
    "disputes": [
      {
        "id": "D1",
        "question": "one-sentence technical question",
        "verdict": "universal" | "clear" | "toss-up",
              # universal = every source independently gave the same proposal and it holds up under verification
              # clear     = the first place pulls ahead of the rest (beats_runner_up names a mechanism)
              # toss-up   = the ranking is still a total order, but the first place does not lead by much;
              #             toss_up_reason says "what evidence is still missing to separate them",
              #             and the orchestrator decides whether to spike / choose by another constraint / ask a human
        "options": [
          {
            "id": "A",
            "tldr": "what it proposes to do",
            "sources": ["reviewer-a", "author"],
            "cost": "what it introduces",
            "verdict": "chosen" | "ranked" | "disqualified",
            "rank": 1,                       # required for chosen/ranked; forbidden for disqualified
            "disqualified_reason": "..."     # required for disqualified
          }
        ],
        "beats_runner_up": "why the first place beats the second",   # required for clear with >= 2 survivors
        "toss_up_reason": "what evidence is still missing to separate them",  # required for toss-up
        "self_selection_note": "on which mechanism the simpler rival loses",  # required when the adjudicator is a source of the chosen option and >= 2 survive
        "simpler_absent_check": "Checked, no simpler form",     # required when all candidates are non-simple
        "failure_trigger": "what counts as the first place failing",    # required for technical disputes
        "outcome_log": ["2026-09-16 A: failed, <observation>"]   # optional, appended by `attempt`
      }
    ]
  }
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import date
from pathlib import Path

from best_of_n_validate import CLEAR, TOSS_UP, UNIVERSAL, LedgerError, validate


def _survivors(dispute: dict) -> list[dict]:
    return sorted(
        (o for o in dispute.get("options", []) if o.get("verdict") != "disqualified"),
        key=lambda o: o["rank"],
    )


def render(ledger: dict) -> str:
    lines = [
        "# Best-of-N adjudication",
        "",
        f"**Locked goal**: {ledger['goal']}",
        f"**Participants**: {', '.join(ledger.get('sources', [])) or '—'}  **Adjudicator**: {ledger.get('adjudicator', '—')}",
        "",
    ]
    all_sources = set(ledger.get("sources") or [])
    for dispute in ledger["disputes"]:
        lines.append(f"## {dispute['id']}: {dispute['question']}")
        lines.append("")
        lines += ["| Rank | Option | Proposal | Cost | Sources |", "|---|---|---|---|---|"]
        for option in _survivors(dispute):
            mark = "**1 ✓**" if option["verdict"] == "chosen" else str(option["rank"])
            lines.append(
                f"| {mark} | {option['id']} | {option['tldr']} | "
                f"{option.get('cost', '—')} | {', '.join(option.get('sources', [])) or '—'} |"
            )
        lines.append("")

        disqualified = [o for o in dispute["options"] if o.get("verdict") == "disqualified"]
        if disqualified:
            lines.append("**Disqualified** (not ranked):")
            for option in disqualified:
                lines.append(f"- {option['id']} — {option['tldr']} | {option['disqualified_reason']}")
            lines.append("")

        chosen = next(o for o in _survivors(dispute) if o["verdict"] == "chosen")
        verdict = dispute.get("verdict")
        if verdict == UNIVERSAL:
            lines += [
                f"**Conclusion: unanimously recommended {chosen['id']}** —— {len(all_sources)} sources independently gave the same proposal, and it holds up under verification.",
                "",
            ]
        elif verdict == CLEAR:
            lines += [
                f"**Conclusion: {chosen['id']} clearly wins** —— no other option independently reached it.",
                "",
            ]
        else:
            live = ", ".join(o["id"] for o in _survivors(dispute))
            lines += [
                f"**Conclusion: toss-up between {live}** —— "
                f"{chosen['id']} did not pull ahead of the rest, but the ranking is still a total order, "
                f"**the next step is still to do {chosen['id']} first**; no need to stop here. "
                "The difference is that the second place is genuinely alive: once the first place goes badly, switch early, do not force it.",
                "",
            ]
        if dispute.get("beats_runner_up"):
            lines += [f"**Why the first place beats the second**: {dispute['beats_runner_up']}", ""]
        # Only mark it when the adjudicator really is a source of the chosen option; otherwise this line states something that did not happen
        if dispute.get("self_selection_note") and ledger.get("adjudicator") in set(chosen.get("sources") or []):
            lines += [
                f"**The adjudicator chose their own approach** ({ledger.get('adjudicator')}): {dispute['self_selection_note']}",
                "",
            ]
        if dispute.get("toss_up_reason"):
            lines += [f"**What is still missing to separate them**: {dispute['toss_up_reason']}", ""]
        lines += [f"**Simpler-form check**: {dispute['simpler_absent_check']}", ""]
        lines += [f"**What counts as failure**: {dispute['failure_trigger']}", ""]
        for entry in dispute.get("outcome_log", []):
            lines.append(f"- {entry}")
        if dispute.get("outcome_log"):
            lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def _dispute(ledger: dict, dispute_id: str) -> dict:
    dispute = next((d for d in ledger["disputes"] if d.get("id") == dispute_id), None)
    if dispute is None:
        raise LedgerError(f"no dispute {dispute_id} in the record")
    return dispute


def _attempted_ids(dispute: dict) -> set[str]:
    """Option ids already attempted according to outcome_log.

    This used to take the **whole** segment before the first `:`, so the schema's own example
    `"2026-09-16 A failed: <observation>"` parsed as `2026-09-16 A failed` — which never matches any
    option, so on switching an option that had already failed was recommended again unchanged. Split
    the prefix on whitespace and commas and recognize known option ids, so both `A: ...` and the
    dated form are recognized.
    """

    known = {str(option.get("id")) for option in dispute.get("options", [])}
    attempted: set[str] = set()
    for entry in dispute.get("outcome_log", []):
        head = str(entry).split(":", 1)[0]
        for token in re.split(r"[\s,]+", head.strip()):
            if token in known:
                attempted.add(token)
    return attempted


def seeds(ledger: dict, dispute_id: str, failed_id: str) -> str:
    """What is left after a failure to serve as the seeds of the second round.

    Deliberately returns **all** surviving options rather than just the next place: the first failed
    attempt is itself new evidence; it may reorder 2..N, or make the one originally third obviously
    right. Compressing it into "pop the head of the queue" loses that. It is also what the orchestrator
    needs — it does not hold technical details and only needs to know "1 of 5 tried, 4 left as seeds";
    the details are judged by the agent that receives this list.
    """

    dispute = _dispute(ledger, dispute_id)
    survivors = _survivors(dispute)
    failed = next((o for o in survivors if o["id"] == failed_id), None)
    if failed is None:
        raise LedgerError(f"option {failed_id} is not in the ranking of dispute {dispute_id}")
    attempted = {failed_id} | _attempted_ids(dispute)
    remaining = [o for o in survivors if o["id"] not in attempted]

    lines = [
        f"DISPUTE={dispute_id}  FAILED={failed_id}  REMAINING={len(remaining)}",
        f"Question: {dispute['question']}",
        f"Failure criterion: {dispute['failure_trigger']}",
    ]
    for entry in dispute.get("outcome_log", []):
        lines.append(f"Already happened: {entry}")
    lines.append("")

    if not remaining:
        lines += [
            "EXHAUSTED —— no options remain in the ranking.",
            "Do not casually invent a new approach: it did not go through this round's eligibility check, nor was it challenged by anyone, "
            "yet it looks more promising than the one that just failed because \"it is new\". Go back to the problem itself and gather evidence again.",
        ]
        return "\n".join(lines)

    lines.append("Remaining options (seeds of the second round, not a queue waiting to be popped):")
    for option in remaining:
        lines.append(f"  [{option['rank']}] {option['id']} — {option['tldr']}")
        lines.append(f"      cost: {option.get('cost', '—')} | sources: {', '.join(option.get('sources', [])) or '—'}")
    lines += [
        "",
    ]
    if dispute.get("verdict") == TOSS_UP:
        # Toss-up means the first place never pulled ahead; the guide's instruction for it is "the second
        # place is genuinely alive; once the first place goes badly, switch early, do not force it".
        # Applying clear's "was the reason overturned" would steer it to "go back to the problem and
        # gather evidence" — exactly the opposite of switching when it is time to switch.
        lines += [
            "This dispute was judged toss-up: the first place never pulled ahead, and the second place is genuinely alive.",
            "Execute the next place directly; do not go gather evidence first — \"switch early, do not force it\" is exactly the toss-up instruction.",
            f"What was missing to separate them back then: {dispute.get('toss_up_reason', '(not recorded)')}",
            "If this failure happens to supply that evidence, reorder the remaining options by it before choosing, and write the reordering into the record.",
        ]
    else:
        lines += [
            f"Why the first place won back then: {dispute.get('beats_runner_up', '(not recorded)')}",
            "",
            "First judge whether that reason has been overturned by this failure:",
            "  overturned → the remaining options largely still hold in their original order; take the one ranked highest;",
            "  not overturned → the root cause may not be \"the wrong approach was chosen\". Do not slide down the ranking; go back to the problem itself and gather evidence.",
            "The failure itself is new evidence: it can reorder the remaining options, or make one of them obviously right. Write any reordering into the record, do not only change it in your head.",
        ]
    return "\n".join(lines)


def record_attempt(ledger: dict, dispute_id: str, option_id: str, outcome: str) -> dict:
    """Append the result of one attempt to the record — the orchestrator's situational awareness is carried by the file, not by its own memory."""

    dispute = _dispute(ledger, dispute_id)
    if not any(o["id"] == option_id for o in dispute.get("options", [])):
        raise LedgerError(f"no option {option_id} in dispute {dispute_id}")
    if not outcome.strip():
        raise LedgerError("outcome must not be empty")
    stamp = date.today().isoformat()
    dispute.setdefault("outcome_log", []).append(f"{stamp} {option_id}: {outcome.strip()}")
    return ledger


def _write_atomic(path: Path, text: str) -> None:
    """Write a temporary file in the same directory first, then replace atomically.

    `write_text` truncates before writing: if the process exits midway it leaves an empty or truncated
    ledger, and the ranking and `outcome_log` are exactly the only fallback after the first place fails
    — with that record gone, `next` can only report it invalid.
    The temporary file must be in the same directory; `replace` across file systems is not atomic.
    """

    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp")
    tmp.write_text(text)
    tmp.replace(path)


def _load(path: Path) -> dict:
    try:
        data = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise LedgerError(f"cannot read ledger: {error}") from error
    if not isinstance(data, dict):
        raise LedgerError("the ledger top level must be an object")
    return data


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)

    rec = sub.add_parser("record", help="validate the ledger and render it deterministically as Markdown")
    rec.add_argument("--ledger", type=Path, required=True)
    rec.add_argument("--output", type=Path, required=True)

    nxt = sub.add_parser("next", help="after a failure, list all remaining options as seeds of the second round")
    nxt.add_argument("--ledger", type=Path, required=True)
    nxt.add_argument("--dispute", required=True)
    nxt.add_argument("--failed", required=True)

    att = sub.add_parser("attempt", help="append the result of one attempt to the record")
    att.add_argument("--ledger", type=Path, required=True)
    att.add_argument("--dispute", required=True)
    att.add_argument("--option", required=True)
    att.add_argument("--outcome", required=True)

    args = parser.parse_args(argv)
    try:
        ledger = _load(args.ledger)
        if args.command == "record":
            if args.output.resolve() == args.ledger.resolve():
                # The same path would replace the JSON ledger in place with Markdown: the ranking and
                # outcome_log vanish together, and that is the only thing this skill must preserve;
                # `attempt` / `next` cannot recover it afterwards.
                raise LedgerError(
                    f"--output and --ledger are the same file ({args.ledger}): "
                    "the rendered result would overwrite the adjudication record itself. Give the rendered output another path"
                )
            errors = validate(ledger)
            if errors:
                print("FAIL")
                for error in errors:
                    print(f"- {error}")
                return 1
            _write_atomic(args.output, render(ledger))
            print(f"PASS\n{args.output}")
            return 0
        if args.command == "attempt":
            # attempt is the **writer**: appending outcome_log to a structurally invalid record without
            # validating first persists the failure history into a file that was never valid, and switch
            # deduplication depends on it. Only the next record would notice, by which time it is written.
            errors = validate(ledger)
            if errors:
                print("FAIL")
                for error in errors:
                    print(f"- {error}")
                return 1
            updated = record_attempt(ledger, args.dispute, args.option, args.outcome)
            _write_atomic(args.ledger, json.dumps(updated, ensure_ascii=False, indent=2) + "\n")
            print(f"RECORDED {args.dispute}/{args.option}")
            return 0
        # next used to read rank/tldr directly, so a corrupt ledger became a KeyError traceback, and
        # switching is used precisely after something went wrong — the last time you want a crash.
        errors = validate(ledger)
        if errors:
            print("FAIL")
            for error in errors:
                print(f"- {error}")
            return 1
        print(seeds(ledger, args.dispute, args.failed))
        return 0
    except LedgerError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
