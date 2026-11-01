#!/usr/bin/env python3
"""Render a sitrep JSON object against computed state; see sitrep-format.md."""
import argparse
import json
import re
import sys

STATUSES = {"planning", "in-development", "test", "review", "blocked"}
SHA = r"[0-9a-fA-F]{40}"
PR_LINK = r"\[#\d+ (?:\\[^\n]|[^\[\]\\\n]|\[(?:\\[^\n]|[^\[\]\\\n])*\])+\]\(https?://[^\s)]+\)"
PR = re.compile(rf"(?:{PR_LINK}|not opened yet \(branch: [^)\n]+\))\Z")
OPEN_PR = re.compile(rf"{PR_LINK}\Z")
LANDED_COMMIT = re.compile(rf"landed {SHA}\Z")


def line(value):
    if not isinstance(value, str) or not value.strip() or any(ord(c) < 32 or 127 <= ord(c) <= 159 or c in "\u2028\u2029" for c in value):
        raise ValueError("fields must be nonempty single-line strings")
    return value.strip()


def computed_state(value):
    if not isinstance(value, dict) or set(value) != {"default_ref", "head", "finished", "open_prs"}:
        raise ValueError("state requires exactly default_ref, head, finished and open_prs")
    if not isinstance(value["default_ref"], str) or not re.fullmatch(r"origin/\S+", value["default_ref"]):
        raise ValueError("state default_ref must be an origin branch")
    if not isinstance(value["head"], str) or not re.fullmatch(SHA, value["head"]):
        raise ValueError("state head must be a full commit SHA")
    if not isinstance(value["finished"], list) or not isinstance(value["open_prs"], list):
        raise ValueError("state finished and open_prs must be arrays")
    finished = []
    for item in value["finished"]:
        if not isinstance(item, dict) or set(item) != {"name", "pr"}:
            raise ValueError("state finished entries require exactly name and pr")
        name, pr = line(item["name"]), line(item["pr"])
        if not LANDED_COMMIT.fullmatch(pr):
            raise ValueError("state finished entries require landed commits")
        finished.append((name, pr))
    open_prs = []
    for item in value["open_prs"]:
        link = line(item)
        if not OPEN_PR.fullmatch(link):
            raise ValueError("state open_prs entries must be PR links")
        open_prs.append(link)
    return finished, open_prs


def render(data, state):
    expected_finished, expected_open = computed_state(state)
    fields = {"ongoing": ("name", "tldr", "context", "status", "pr"),
              "finished": ("name", "tldr", "context", "pr"),
              "deferred": ("name", "tldr", "context")}
    if not isinstance(data, dict) or set(data) != {"blockers", *fields}:
        raise ValueError("expected exactly blockers, ongoing, finished and deferred")
    if any(not isinstance(data[k], list) for k in data):
        raise ValueError("sections must be arrays")
    blockers = [line(item) for item in data["blockers"]]
    if any(re.match(r"(?i)^none(?:\b|$)", item) for item in blockers):
        raise ValueError("use an empty blockers array for None")
    sections = ["## Blockers\n" + ("\n".join("- " + item for item in blockers) if blockers else "None")]
    titles = {"ongoing": "Ongoing", "finished": "Finished (since the last sitrep)", "deferred": "Deferred"}
    labels = {"tldr": "TLDR", "context": "Context", "status": "Status", "pr": "PR/MR"}
    actual_finished = []
    actual_open = []
    for key, keys in fields.items():
        rows = []
        for item in data[key]:
            if not isinstance(item, dict) or set(item) != set(keys):
                raise ValueError(f"{key} items require {', '.join(keys)}")
            values = {k: line(item[k]) for k in keys}
            if "status" in values and values["status"] not in STATUSES:
                raise ValueError("invalid status")
            if key == "finished":
                if not LANDED_COMMIT.fullmatch(values["pr"]):
                    raise ValueError("finished items require landed commits from computed state")
                actual_finished.append((values["name"], values["pr"]))
            elif "pr" in values:
                if not PR.fullmatch(values["pr"]):
                    raise ValueError("invalid PR/MR link or branch")
                if OPEN_PR.fullmatch(values["pr"]):
                    actual_open.append(values["pr"])
            rows.append("- " + values["name"] + "\n" + "\n".join(
                f"  - {labels[k]}: {values[k]}" for k in keys[1:]))
        sections.append("## " + titles[key] + "\n" + ("\n".join(rows) if rows else "None"))
    if actual_finished != expected_finished:
        raise ValueError("finished items do not match the computed Git log")
    if actual_open != expected_open:
        raise ValueError("ongoing PRs do not match the computed open PR list")
    return "\n\n".join(sections) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", required=True, type=argparse.FileType("r", encoding="utf-8"))
    args = parser.parse_args()
    try:
        output = render(json.load(sys.stdin), json.load(args.state))
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
        print(f"sitrep: {error}", file=sys.stderr)
        return 1
    print(output, end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
