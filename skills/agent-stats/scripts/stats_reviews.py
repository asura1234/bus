"""Canonical review adaptation and shared semantic identity for stored facts."""

import hashlib
import re
import sys
from pathlib import Path


def canonical_review(path, text):
    support = Path(__file__).resolve().parents[2] / "_bus/cli_extensions"
    if not (support / "review_artifact_parser.py").is_file():
        raise ValueError("canonical review parser unavailable")
    sys.path.insert(0, str(support))
    from review_artifact_parser import parse_review_artifact

    artifact = parse_review_artifact(Path(path))
    if artifact.mode != "pr" or artifact.text != text.replace("\r\n", "\n").replace(
        "\r", "\n"
    ):
        raise ValueError("review changed during canonical parsing")
    return artifact


def review_scope(row):
    data = row["data"]
    if "scope_hash" in data:
        return data["scope_hash"]
    # Pre-scope event records retain their original source bytes. Recover only
    # an explicit header, never infer scope from a path or reviewer name.
    src = row["sources"][0]
    raw = Path(src["path"]).read_bytes()
    if hashlib.sha256(raw).hexdigest() != src["sha256"]:
        raise ValueError("legacy review source checksum changed")
    text = raw.decode("utf-8")
    header = text.split("\n## ", 1)[0]
    values = re.findall(r"^\*\*范围哈希\*\*[：:]\s*([0-9a-f]{64})\s*$", header, re.M)
    if len(values) > 1:
        raise ValueError("conflicting legacy review scopes")
    return values[0] if values else None


def review_data(row):
    return dict(row["data"], scope_hash=review_scope(row))


def review_key(row):
    data = row["data"]
    return (
        row["task"],
        row["agent"]["name"],
        review_scope(row),
        data["round"],
        data["head"],
    )


def validate_reviews(rows):
    seen = {}
    for row in rows:
        if row["kind"] != "review":
            continue
        key, data = review_key(row), review_data(row)
        if key in seen and seen[key] != data:
            raise ValueError("conflicting stored review facts")
        seen[key] = data


def source_reference(lane, round_, finding, reviews, scope_hash=None):
    candidates = [
        r[1]
        for r in reviews
        if r[0] == lane
        and r[1]["round"] == round_
        and (scope_hash is None or r[1].get("scope_hash") == scope_hash)
        and any(f["id"] == finding for f in r[1]["findings"])
    ]
    detailed = [r for r in candidates if r.get("head")]
    identities = {(r.get("scope_hash"), r.get("head")) for r in detailed or candidates}
    exact = len(identities) == 1
    return dict(
        reviewer=lane,
        round=round_ if exact else None,
        finding_id=finding,
        scope_hash=next(iter(identities))[0] if exact else None,
    )
