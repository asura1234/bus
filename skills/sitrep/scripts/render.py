#!/usr/bin/env python3
"""Render a sitrep JSON object; the format contract is in sitrep-format.md."""
import json
import re
import subprocess
import sys

STATUSES = {"planning", "in-development", "test", "review", "blocked"}
PR_LINK = r"\[#\d+ (?:\\[^\n]|[^\[\]\\\n]|\[(?:\\[^\n]|[^\[\]\\\n])*\])+\]\(https?://[^\s)]+\)"
PR = re.compile(rf"(?:{PR_LINK}|not opened yet \(branch: [^)\n]+\))\Z")
MERGED_PR = re.compile(rf"merged {PR_LINK}\Z")
LANDED_COMMIT = re.compile(r"landed [0-9a-fA-F]{40}\Z")


def command(args):
    try:
        return subprocess.run(args, text=True, capture_output=True, check=False, timeout=60)
    except (OSError, subprocess.SubprocessError) as error:
        raise ValueError(f"could not run {args[0]} state lookup: {error}") from error


def github_pr_merged(url):
    result = command(["gh", "pr", "view", url, "--json", "state"])
    if result.returncode != 0:
        raise ValueError(f"gh could not verify finished PR: {result.stderr.strip()}")
    try:
        state = json.loads(result.stdout)["state"]
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise ValueError("gh returned an invalid PR state") from error
    return state == "MERGED"


def git_commit_landed(sha):
    fetched = command(["git", "fetch", "origin"])
    if fetched.returncode != 0:
        raise ValueError(f"git could not fetch origin: {fetched.stderr.strip()}")
    default = command(["git", "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
    if default.returncode == 0:
        default_ref = default.stdout.strip()
    else:
        remote = command(["git", "remote", "show", "origin"])
        match = re.search(r"(?m)^\s*HEAD branch: (\S+)\s*$", remote.stdout)
        if remote.returncode != 0 or not match:
            raise ValueError("git could not identify the default branch")
        default_ref = f"origin/{match.group(1)}"
    landed = command(["git", "merge-base", "--is-ancestor", sha, default_ref])
    if landed.returncode == 1:
        return False
    if landed.returncode != 0:
        raise ValueError(f"git could not verify landed commit: {landed.stderr.strip()}")
    return True


def line(value):
    if not isinstance(value, str) or not value.strip() or any(ord(c) < 32 or 127 <= ord(c) <= 159 or c in "\u2028\u2029" for c in value):
        raise ValueError("fields must be nonempty single-line strings")
    return value.strip()


def render(data, pr_merged=None, commit_landed=None):
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
            if key == "finished":
                if MERGED_PR.fullmatch(values["pr"]):
                    url = values["pr"].rsplit("](", 1)[1][:-1]
                    if not (pr_merged or github_pr_merged)(url):
                        raise ValueError("finished PR is not MERGED according to gh")
                elif LANDED_COMMIT.fullmatch(values["pr"]):
                    sha = values["pr"].split()[1]
                    if not (commit_landed or git_commit_landed)(sha):
                        raise ValueError("finished commit is not on the default branch")
                else:
                    raise ValueError("finished item requires a merged PR link and title or a landed commit")
            elif "pr" in values and not PR.fullmatch(values["pr"]):
                raise ValueError("invalid PR/MR link or branch")
            rows.append("- " + values["name"] + "\n" + "\n".join(
                f"  - {labels[k]}: {values[k]}" for k in keys[1:]))
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
