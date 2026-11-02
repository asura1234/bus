"""Read recorded review, triage and runner facts; never adjudicate findings."""

import re


def unfenced(text):
    lines = []
    fence = None
    for line in text.splitlines():
        marker = re.match(r"^\s*(`{3,}|~{3,})", line)
        if marker:
            if fence is None:
                fence = marker.group(1)[0]
            elif marker.group(1)[0] == fence:
                fence = None
            lines.append("")
        else:
            lines.append(line if fence is None else "")
    return "\n".join(lines)


def section(text, title):
    match = re.search(
        r"^## " + re.escape(title) + r"\s*\n(.*?)(?=^## |\Z)", text, re.M | re.S
    )
    return match.group(1).strip() if match else None


def field(text, name, required=True):
    found = re.findall(r"^\*\*" + re.escape(name) + r"\*\*[：:]\s*(.+)$", text, re.M)
    if len(found) != 1:
        if not required and not found:
            return None
        raise ValueError(f"invalid artifact field {name}")
    return found[0].strip()


def review(text):
    text = unfenced(text)
    match = re.match(r"^# Review Round (\d+) — 代码审查\s*$", text.splitlines()[0])
    if not match:
        raise ValueError("invalid review round header")
    target = re.fullmatch(r"(.+?) @ ([0-9a-f]{7,64})", field(text, "分支"))
    if not target:
        raise ValueError("invalid review branch/head")
    substantive = section(text, "新问题与建议")
    if (
        substantive is None
        or section(text, "同步清单（CONSISTENCY drift，非阻塞）") is None
    ):
        raise ValueError("invalid review: missing actionable sections")
    verdict = re.findall(
        r"^- \*\*判定\*\*[：:]\s*(Ready|Needs Refinement|Abandon)\b", text, re.M
    )
    if len(verdict) != 1:
        raise ValueError("invalid review verdict")
    findings = []
    for heading, body in re.findall(
        r"^###\s+(\d+[.、].*?)\n(.*?)(?=^### |\Z)", substantive, re.M | re.S
    ):

        def item(name):
            m = re.search(r"^- \*\*" + name + r"\*\*[：:]\s*(.+)$", body, re.M)
            return m.group(1).strip() if m else None

        findings.append(
            dict(
                id=re.match(r"\d+", heading).group(),
                dimension=item("维度"),
                location=item("位置"),
                inherited=bool(re.search(r"承 R\d+-\d+", heading)),
            )
        )
    if not findings and substantive not in ("无。", "无", "None.", "None"):
        raise ValueError("invalid review finding structure")
    return (
        field(text, "审查者"),
        dict(
            round=int(match.group(1)),
            head=target.group(2),
            branch=target.group(1),
            date=field(text, "日期"),
            verdict=verdict[0],
            findings=findings,
        ),
        None,
    )


def triage(text, lanes, reviews, runs=None):
    """Keep exactly the canonical items and corroborating lanes the author recorded."""
    text = unfenced(text)
    structured = section(text, "Root cause provenance")
    if structured is not None:
        items = []
        for line in structured.splitlines():
            cells = [x.strip() for x in line.strip().strip("|").split("|")]
            if (
                not line.startswith("|")
                or cells[0] == "claim_id"
                or cells[0].startswith("-")
            ):
                continue
            if len(cells) != 3:
                raise ValueError("invalid root provenance table")
            claim, outcome, sources = cells
            refs, run_ids = [], []
            for raw in sources.split(","):
                if raw.strip().startswith("run@"):
                    pointer = raw.strip()[4:]
                    if "#" not in pointer:
                        raise ValueError("invalid run source reference")
                    path, test_id = pointer.split("#", 1)
                    matched = (runs or {}).get(path)
                    if (
                        not matched
                        or matched["data"]["result"] != "FAIL"
                        or test_id not in matched["data"]["failing_test_ids"]
                    ):
                        raise ValueError(
                            "invalid run source: missing failed test evidence"
                        )
                    run_ids.append(matched["id"])
                    continue
                m = re.fullmatch(r"([\w-]+)@(\d+):([\w-]+)", raw.strip())
                if not m:
                    raise ValueError("invalid root source reference")
                lane, round_, finding = m.groups()
                if not any(
                    r[0] == lane
                    and r[1]["round"] == int(round_)
                    and any(f["id"] == finding for f in r[1]["findings"])
                    for r in reviews
                ):
                    raise ValueError(f"invalid root source: {raw}")
                refs.append(dict(reviewer=lane, round=int(round_), finding_id=finding))
            if outcome not in ("APPLY", "REJECT", "FLAG", "WITHDRAWN"):
                raise ValueError("invalid root outcome")
            body = section(text, outcome)
            if body is None or not re.search(
                r"(?m)^\s*(?:- |\d+\. )?" + re.escape(claim) + r"\b", body
            ):
                raise ValueError("invalid root outcome: absent canonical claim")
            canonical_item = re.search(
                r"(?m)^(?:- |\d+\. )" + re.escape(claim) + r"\b.*$", body
            )
            locations = re.findall(
                r"(?<![\w/])([\w./-]+):\d+",
                canonical_item.group() if canonical_item else "",
            )
            items.append((claim, outcome, refs, run_ids, locations))
        if len({x[0] for x in items}) != len(items):
            raise ValueError("invalid duplicate canonical claim")
        return items
    items = []
    # Canonical historical items are top-level list entries. Continuations are evidence,
    # not additional claims. No title/location similarity deduplication is performed.
    for outcome in ("APPLY", "REJECT", "FLAG", "WITHDRAWN"):
        body = section(text, outcome) or ""
        for line in body.splitlines():
            m = re.match(r"^(?:- ([A-Za-z][\w-]*)\b|([0-9]+)\. )", line)
            if not m:
                continue
            claim = m.group(1) or m.group(2)
            refs = []
            for lane in sorted(lanes, key=lambda x: (-len(x), x)):
                lm = re.search(
                    r"(?<![\w-])" + re.escape(lane) + r"(?=$|[^A-Za-z_-])", line
                )
                if lm:
                    suffix = line[lm.end() :]
                    fm = re.match(r"(?:\s+finding\s+|\s*#?)(\d+)\b", suffix)
                    finding = fm.group(1) if fm else None
                    candidates = [
                        r
                        for r in reviews
                        if r[0] == lane
                        and finding
                        and any(f["id"] == finding for f in r[1]["findings"])
                    ]
                    rounds = {r[1]["round"] for r in candidates}
                    refs.append(
                        dict(
                            reviewer=lane,
                            round=next(iter(rounds)) if len(rounds) == 1 else None,
                            finding_id=finding,
                        )
                    )
            if refs:
                items.append(
                    (claim, outcome, sorted(refs, key=lambda x: x["reviewer"]), [], [])
                )
    return items


def runner(value):
    if not isinstance(value, dict):
        return None
    if (
        value.get("kind") in ("gate", "e2e", "live")
        and {"head", "result", "started_at", "finished_at"} <= value.keys()
    ):
        return dict(
            kind=value["kind"],
            head=value["head"],
            result=value["result"],
            duration_ms=value.get("duration_ms"),
            failure_cause=value.get("failure_cause"),
            full=value.get("full"),
            lanes=value.get("lanes", []),
            started_at=value["started_at"],
            finished_at=value["finished_at"],
            slot_wait_ms=value.get("slot_wait_ms"),
            failing_test_ids=value.get("failing_test_ids", []),
        )
    if {"head", "duration_ms", "exit_code", "results"} <= value.keys():
        if (
            type(value["exit_code"]) is not int
            or type(value["duration_ms"]) is not int
            or value["duration_ms"] < 0
        ):
            raise ValueError("invalid runner result/duration")
        lanes = []
        for entry in value["results"]:
            lanes.append(
                dict(
                    name=entry["name"],
                    result="PASS" if entry["exit_code"] == 0 else "FAIL",
                    duration_ms=entry["duration_ms"],
                )
            )
        return dict(
            kind="gate",
            head=value["head"],
            result="PASS" if value["exit_code"] == 0 else "FAIL",
            duration_ms=value["duration_ms"],
            failure_cause=value.get("failure_cause"),
            full=value.get("selection", {}).get("full"),
            lanes=lanes,
            started_at=value.get("started_at"),
            finished_at=value.get("finished_at"),
            slot_wait_ms=value.get("slot_wait_ms"),
            failing_test_ids=value.get("failing_test_ids", []),
        )
    return None
