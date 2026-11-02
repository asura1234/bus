"""Serialize existing canonical adjudications; never deduplicate or rate findings."""

import argparse
import json
import re
from pathlib import Path

OUTCOMES = ("APPLY", "REJECT", "FLAG", "WITHDRAWN")
TITLE = "Root cause provenance"


def _plain(text):
    result, fence = [], None
    for line in text.splitlines():
        marker = re.match(r"^\s*(`{3,}|~{3,})", line)
        if marker:
            value = marker.group(1)
            if fence is None:
                fence = value
            elif value[0] == fence[0] and len(value) >= len(fence):
                fence = None
            result.append("")
        else:
            result.append(line if fence is None else "")
    if fence is not None:
        raise ValueError("unterminated triage code fence")
    return "\n".join(result)


def _section(text, name):
    matches = re.findall(
        r"^## " + re.escape(name) + r"\s*\n(.*?)(?=^## |\Z)", text, re.M | re.S
    )
    if len(matches) > 1:
        raise ValueError("duplicate triage section: " + name)
    return matches[0] if matches else None


def _validate(claims, text):
    if type(claims) is not list:
        raise ValueError("canonical claims must be an array")
    seen = set()
    for claim in claims:
        if type(claim) is not dict or set(claim) != {"claim_id", "outcome", "sources"}:
            raise ValueError("canonical claim requires only claim_id/outcome/sources")
        cid, outcome, sources = (claim[k] for k in ("claim_id", "outcome", "sources"))
        if (
            not isinstance(cid, str)
            or not re.fullmatch(r"[A-Za-z0-9][\w-]*", cid)
            or cid in seen
        ):
            raise ValueError("invalid or duplicate canonical claim id")
        seen.add(cid)
        if outcome not in OUTCOMES:
            raise ValueError("invalid canonical outcome")
        if type(sources) is not list or any(
            type(x) is not str or not x for x in sources
        ):
            raise ValueError("canonical sources must be strings")
        if len(sources) != len(set(sources)):
            raise ValueError("duplicate canonical source")
        locations = [
            name
            for name in OUTCOMES
            if re.search(
                r"(?m)^(?:- )?" + re.escape(cid) + r"(?![\w-])",
                _section(text, name) or "",
            )
        ]
        if locations != [outcome]:
            raise ValueError("canonical outcome disagrees with triage: " + cid)
    return claims


def parse_provenance(text):
    """Public reader for the owning skill's canonical provenance section."""
    text = _plain(text)
    body = _section(text, TITLE)
    if body is None:
        return None
    result = []
    lines = [line for line in body.splitlines() if line.strip()]

    def row_cells(line):
        return [x.strip() for x in line.strip().strip("|").split("|")]

    if len(lines) < 2 or row_cells(lines[0]) != ["claim_id", "outcome", "sources"]:
        raise ValueError("invalid canonical provenance columns")
    if len(row_cells(lines[1])) != 3 or not all(
        re.fullmatch(r":?-{3,}:?", x) for x in row_cells(lines[1])
    ):
        raise ValueError("invalid canonical provenance separator")
    for line in lines[2:]:
        cells = [x.strip() for x in line.strip().strip("|").split("|")]
        if not line.startswith("|") or len(cells) != 3:
            raise ValueError("invalid canonical provenance table")
        cid, outcome, raw = cells
        sources = (
            json.loads(raw)
            if raw.startswith("[")
            else [x.strip() for x in raw.split(",") if x.strip()]
        )
        result.append(dict(claim_id=cid, outcome=outcome, sources=sources))
    return _validate(result, text)


def render_provenance(claims, triage):
    """Project main's already-deduplicated decisions without making new ones."""
    _validate(claims, _plain(triage))
    lines = ["## " + TITLE, "", "| claim_id | outcome | sources |", "|---|---|---|"]
    for claim in sorted(claims, key=lambda x: x["claim_id"]):
        sources = json.dumps(
            sorted(claim["sources"]), ensure_ascii=False, separators=(",", ":")
        ).replace("|", "\\u007c")
        lines.append(
            "| " + claim["claim_id"] + " | " + claim["outcome"] + " | " + sources + " |"
        )
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claims", type=Path, required=True)
    parser.add_argument("--triage", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.resolve() in (args.claims.resolve(), args.triage.resolve()):
        parser.error("output must not overwrite source decisions")
    try:
        output = render_provenance(
            json.loads(args.claims.read_text()), args.triage.read_text()
        )
        args.output.write_text(output)
    except (OSError, ValueError) as error:
        parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
