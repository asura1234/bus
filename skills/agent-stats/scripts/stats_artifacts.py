"""Read recorded review, triage and runner facts; never adjudicate findings."""

import re
from stats_reviews import source_reference


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


def review(text, path):
    from stats_reviews import canonical_review

    artifact = canonical_review(path, text)
    text = unfenced(artifact.text)
    target = re.fullmatch(r"(.+?) @ ([0-9a-f]{7,64})", artifact.target["branch"])
    if not target:
        raise ValueError("invalid review branch/head")
    substantive = unfenced(artifact.substantive)
    findings = []
    blocks = re.findall(r"^###\s+(.+?)\n(.*?)(?=^### |\Z)", substantive, re.M | re.S)
    if len(blocks) != len(artifact.finding_ids):
        raise ValueError("invalid review finding structure")
    for finding_id, (heading, body) in zip(artifact.finding_ids, blocks):

        def item(name):
            m = re.search(r"^- \*\*" + name + r"\*\*[：:]\s*(.+)$", body, re.M)
            return m.group(1).strip() if m else None

        findings.append(
            dict(
                id=finding_id,
                dimension=item("维度"),
                location=item("位置"),
                inherited=bool(re.search(r"承 R\d+-\d+", heading)),
            )
        )
    if [f["id"] for f in findings] != list(artifact.finding_ids):
        raise ValueError("invalid review finding structure")
    return (
        artifact.lane,
        dict(
            round=artifact.round_number,
            scope_hash=artifact.target.get("scope_hash"),
            head=target.group(2),
            branch=target.group(1),
            date=field(text, "日期"),
            verdict="Abandon"
            if artifact.verdict.startswith("Abandon")
            else artifact.verdict,
            findings=findings,
        ),
        None,
    )


def owned_provenance(text):
    import sys
    from pathlib import Path

    scripts = Path(__file__).resolve().parents[2] / "address-review-comments/scripts"
    if not (scripts / "root_provenance.py").is_file():
        raise ValueError("canonical root provenance reader unavailable")
    sys.path.insert(0, str(scripts))
    from root_provenance import parse_provenance, provenance_scope

    return parse_provenance(text), provenance_scope(text)


def triage(text, lanes, reviews, runs=None):
    """Keep exactly the canonical items and corroborating lanes the author recorded."""
    structured, scope_hash = owned_provenance(text)
    text = unfenced(text)
    if structured is not None:
        items = []
        for decision in structured:
            claim, outcome, sources = (
                decision[k] for k in ("claim_id", "outcome", "sources")
            )
            refs, run_ids = [], []
            for raw in sources:
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
                    and (scope_hash is None or r[1].get("scope_hash") == scope_hash)
                    and any(f["id"] == finding for f in r[1]["findings"])
                    for r in reviews
                ):
                    raise ValueError(f"invalid root source: {raw}")
                refs.append(
                    source_reference(lane, int(round_), finding, reviews, scope_hash)
                )
            body = section(text, outcome)
            canonical_item = re.search(
                r"(?m)^(?:- )?" + re.escape(claim) + r"(?![\w-]).*$", body
            )
            locations = re.findall(
                r"(?<![\w/])([\w./-]+):\d+",
                canonical_item.group() if canonical_item else "",
            )
            items.append((claim, outcome, refs, run_ids, locations))
        return items
    single = re.search(r"one canonical claim from ([\w-]+) round (\d+)\b", text)
    disposition = re.findall(r"(?m)^(APPLY)-(?:SAFE|BEHAVIOR)\b", text)
    if single and len(disposition) == 1:
        lane, round_ = single.groups()
        ids = {
            f["id"]
            for r in reviews
            if r[0] == lane and r[1]["round"] == int(round_)
            for f in r[1]["findings"]
        }
        if len(ids) == 1:
            return [
                (
                    "sole-claim",
                    "APPLY",
                    [source_reference(lane, int(round_), next(iter(ids)), reviews)],
                    [],
                    [],
                )
            ]
    items = []
    # Canonical historical items are top-level list entries. Continuations are evidence,
    # not additional claims. No title/location similarity deduplication is performed.
    for outcome in ("APPLY", "REJECT", "FLAG", "WITHDRAWN"):
        body = section(text, outcome) or ""
        for line in body.splitlines():
            m = re.match(
                r"^(?:- ([A-Za-z][\w-]*)\b|([0-9]+)\. |([A-Z][A-Z0-9_-]*)\s*/\s*)", line
            )
            if not m:
                continue
            claim = m.group(1) or m.group(2) or m.group(3)
            refs = []
            before, separator, after = line.partition(" — ")
            explicit = any(
                re.search(
                    r"(?<![\w-])" + re.escape(lane) + r"(?=$|[^A-Za-z_-])", before
                )
                for lane in lanes
            )
            source_span = (
                before
                if separator and explicit
                else after.split(";", 1)[0]
                if separator
                else line
            )
            for lane in sorted(lanes, key=lambda x: (-len(x), x)):
                lm = re.search(
                    r"(?<![\w-])" + re.escape(lane) + r"(?=$|[^A-Za-z_-])", source_span
                )
                if lm:
                    suffix = source_span[lm.end() :]
                    explicit = re.match(r"\s+R(\d+)-([\w-]+)\b", suffix)
                    if explicit:
                        round_, finding = int(explicit.group(1)), explicit.group(2)
                        known = [
                            r
                            for r in reviews
                            if r[0] == lane
                            and r[1]["round"] == round_
                            and any(f["id"] == finding for f in r[1]["findings"])
                        ]
                        refs.append(
                            source_reference(lane, round_, finding, reviews, scope_hash)
                            if known
                            else dict(
                                reviewer=lane,
                                round=round_,
                                finding_id=finding,
                                scope_hash=scope_hash,
                            )
                        )
                        continue
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
                        source_reference(lane, next(iter(rounds)), finding, reviews)
                        if len(rounds) == 1
                        else dict(
                            reviewer=lane,
                            round=None,
                            finding_id=finding,
                            scope_hash=None,
                        )
                    )
            if refs or m.group(3):
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
        data = run_defaults(value["kind"], value["head"], value["result"])
        for key in data:
            if key in value:
                data[key] = value[key]
        return data
    if {"head", "duration_ms", "exit_code", "results"} <= value.keys():
        if (
            type(value["exit_code"]) is not int
            or type(value["duration_ms"]) is not int
            or value["duration_ms"] < 0
        ):
            raise ValueError("invalid runner result/duration")
        data = run_defaults(
            "gate", value["head"], "PASS" if value["exit_code"] == 0 else "FAIL"
        )
        data.update(
            duration_ms=value["duration_ms"],
            failure_cause=value.get("failure_cause"),
            full=value.get("selection", {}).get("full"),
            lanes=[
                dict(
                    name=e["name"],
                    result="PASS" if e["exit_code"] == 0 else "FAIL",
                    duration_ms=e["duration_ms"],
                )
                for e in value["results"]
            ],
            started_at=value.get("started_at"),
            finished_at=value.get("finished_at"),
            slot_wait_ms=value.get("slot_wait_ms"),
            failing_test_ids=value.get("failing_test_ids", []),
        )
        return data
    if isinstance(value.get("head"), str):
        if value.get("result") in ("PASS", "FAIL") and (
            "real_provider" in value or "recovered_request" in value
        ):
            return run_defaults("live", value["head"], value["result"])
        if value.get("outcome") in ("PASS", "FAIL") and "native_receipt_exact" in value:
            return run_defaults("live", value["head"], value["outcome"])
        if type(value.get("pass")) is bool and "providers" in value:
            return run_defaults(
                "live", value["head"], "PASS" if value["pass"] else "FAIL"
            )
    return None


def run_defaults(kind, head, result, **fields):
    return dict(
        kind=kind,
        head=head,
        result=result,
        duration_ms=None,
        failure_cause=None,
        full=None,
        lanes=[],
        started_at=None,
        finished_at=None,
        slot_wait_ms=None,
        failing_test_ids=[],
        **fields,
    )


def verified_current_gate(text, artifact=None, decode=False):
    """Use only the owning skill's public verify/show protocol."""
    import subprocess
    import sys
    from contextlib import nullcontext
    from pathlib import Path
    from tempfile import TemporaryDirectory

    script = (
        Path(__file__).resolve().parents[2] / "gate-and-fix/scripts/gate_and_fix.py"
    )
    if not script.is_file():
        raise ValueError("canonical gate command unavailable")
    plain = unfenced(text)
    base = re.search(r"^- Base: `([^`]+)`$", plain, re.M)
    if not base:
        raise ValueError("invalid gate base")
    with (
        TemporaryDirectory(prefix="bus-stats-gate-")
        if artifact is None
        else nullcontext(None) as scratch
    ):
        path = Path(scratch) / "round.md" if artifact is None else Path(artifact)
        if artifact is None:
            path.write_text(text)

        def call(operation, *arguments):
            result = subprocess.run(
                [
                    sys.executable,
                    str(script),
                    operation,
                    "--artifact",
                    str(path),
                    "--base",
                    base.group(1),
                    *arguments,
                ],
                capture_output=True,
            )
            if result.returncode:
                raise ValueError(
                    "canonical gate "
                    + operation
                    + ": "
                    + result.stderr.decode("utf-8", errors="replace")
                )
            return result.stdout.decode("utf-8")

        outcome = call("verify").strip()
        if outcome not in ("PASS", "FAIL"):
            raise ValueError("invalid canonical gate verification result")
        if not decode:
            return outcome
        names = re.findall(r"^### ([a-z][a-z0-9-]*) — (?:PASS|FAIL)$", plain, re.M)
        return [
            call("show", "--gate", name, "--stream", stream)
            for name in names
            for stream in ("stdout", "stderr")
        ]


def markdown_run(text, artifact=None):
    plain = unfenced(text)
    if plain.startswith("# Gate-and-Fix Round\n"):

        def scalar(name):
            matches = re.findall(
                r"^- " + re.escape(name) + r": `([^`]+)`$", plain, re.M
            )
            if len(matches) != 1:
                raise ValueError("invalid gate metadata: " + name)
            return matches[0]

        if scalar("Schema") not in ("2", "3"):
            raise ValueError("invalid gate schema")
        if scalar("Schema") == "3":
            verified_current_gate(text, artifact)
        data = run_defaults("gate", scalar("Head"), scalar("Outcome"))
        for name, result, body in re.findall(
            r"^### (.+?) — (PASS|FAIL)\n(.*?)(?=^### |\Z)", plain, re.M | re.S
        ):
            duration = re.search(r"^- Duration: `(\d+)ms`$", body, re.M)
            if scalar("Schema") == "2":
                exits = re.findall(r"^- Exit code: `([^`]+)`$", body, re.M)
                if len(exits) != 1 or not re.fullmatch(r"-?\d+|launch-error", exits[0]):
                    raise ValueError("invalid legacy gate exit code")
                if (exits[0] == "0") != (result == "PASS"):
                    raise ValueError("legacy gate status disagrees with exit code")
            data["lanes"].append(
                dict(
                    name=name,
                    result=result,
                    duration_ms=int(duration.group(1)) if duration else None,
                )
            )
        if not data["lanes"]:
            raise ValueError("invalid gate: no runner lanes")
        expected = (
            "PASS"
            if all(lane["result"] == "PASS" for lane in data["lanes"])
            else "FAIL"
        )
        if data["result"] != expected:
            raise ValueError("gate aggregate disagrees with lane results")
        return data
    if plain.startswith("# E2E TUI Run\n"):
        head = re.search(r"^- Commit: `([0-9a-f]{7,64})`$", plain, re.M)
        summary = section(text, "Summary") or ""
        fenced = re.fullmatch(r"```(?:text)?\s*\n(.*?)\n```", summary, re.S)
        if fenced:
            summary = fenced.group(1)
        verdict = re.findall(
            r"^Verdict: (GREEN|RED|BLOCKED|INCOMPLETE(?: \(\d+ case\(s\) not fully run\))?)\s*$",
            summary,
            re.M,
        )
        verdict = ["INCOMPLETE" if x.startswith("INCOMPLETE") else x for x in verdict]
        if not head or len(verdict) != 1:
            raise ValueError("invalid E2E runner metadata")
        return run_defaults(
            "e2e",
            head.group(1),
            {
                "GREEN": "PASS",
                "RED": "FAIL",
                "BLOCKED": "INTERRUPTED",
                "INCOMPLETE": "INCOMPLETE",
            }[verdict[0]],
        )
    return None


def wrapped_reports(text, head, candidates, outcome, artifact=None):
    """Link only runner paths actually printed in the gate's encoded output."""
    import base64
    from pathlib import Path

    decoded = []
    if re.search(r"^- Schema: `3`$", unfenced(text), re.M):
        decoded = verified_current_gate(text, artifact, decode=True)
    else:
        for raw in re.findall(r"```base64\n(.*?)\n```", text, re.S):
            decoded.append(base64.b64decode(raw, validate=True).decode("utf-8"))
    output = "\n".join(decoded)
    refs = re.findall(r"Test report: (.+?\.json)\s*$", output, re.M)
    found, missing = [], []
    for ref in refs:
        # Native runner UUID filenames survive archival copies. Other filenames
        # need the exact reference; elapsed time and head are not run identities.
        native_id = re.fullmatch(r"run-[0-9a-f]{32}\.json", Path(ref).name)
        matches = [
            (p, v)
            for p, v in candidates
            if p.resolve() == Path(ref).resolve()
            or (native_id and p.name == Path(ref).name)
        ]
        from stats_schema import digest

        identities = {digest(v) for _, v in matches}
        if len(identities) > 1:
            raise ValueError("ambiguous wrapped runner report")
        if matches:
            p, value = matches[0]
            if value.get("head") != head:
                raise ValueError("wrapped runner head disagrees with gate")
            if outcome == "PASS" and value.get("exit_code") != 0:
                raise ValueError("wrapped runner outcome disagrees with passing gate")
            found.append(p)
        else:
            missing.append(ref)
    return found, missing
