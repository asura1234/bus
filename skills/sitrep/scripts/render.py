#!/usr/bin/env python3
"""Render a sitrep JSON object; the format contract is in sitrep-format.md."""
import json
import re
import sys

STATUSES = {"planning", "in-development", "test", "review", "blocked"}
PR = re.compile(r"(?:\[#\d+ [^\]\n]+\]\(https?://[^\s)]+\)|not opened yet \(branch: [^)\n]+\))\Z")


def line(value):
    if not isinstance(value, str) or not value.strip() or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise ValueError("fields must be nonempty single-line strings")
    return value.strip()


def render(data):
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
    for key, keys in fields.items():
        rows = []
        for item in data[key]:
            if not isinstance(item, dict) or set(item) != set(keys):
                raise ValueError(f"{key} items require {', '.join(keys)}")
            values = {k: line(item[k]) for k in keys}
            if "status" in values and values["status"] not in STATUSES:
                raise ValueError("invalid status")
            if "pr" in values and not PR.fullmatch(values["pr"]):
                raise ValueError("invalid PR/MR link or branch")
            rows.append("- " + values["name"] + "\n" + "\n".join(
                f"    - {labels[k]}: {values[k]}" for k in keys[1:]))
        sections.append("## " + titles[key] + "\n" + ("\n".join(rows) if rows else "None"))
    return "\n\n".join(sections) + "\n"


def main():
    try:
        output = render(json.load(sys.stdin))
    except (ValueError, TypeError, json.JSONDecodeError) as error:
        print(f"sitrep: {error}", file=sys.stderr)
        return 1
    print(output, end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
