"""Count an escape only with ordered detection and unchanged cited Git objects."""

from datetime import datetime
import re
import subprocess


def escapes(rows, repo):
    if repo is None:
        return []
    from stats_collect import git
    from stats_schema import record

    reviews = [r for r in rows if r["kind"] == "review"]
    runs = {r["id"]: r for r in rows if r["kind"] == "run"}
    roots = [
        r for r in rows if r["kind"] == "root_cause" and r["data"]["outcome"] == "APPLY"
    ]
    result = []
    for root in roots:
        detected, locations = [], list(root["data"]["locations"])
        for rid in root["data"]["run_ids"]:
            run = runs.get(rid)
            if (
                run
                and run["data"]["head"] == root["data"]["head"]
                and run["data"]["finished_at"]
            ):
                detected.append(run)
        valid = True
        for ref in root["data"]["source_refs"]:
            candidates = [
                r
                for r in reviews
                if r["agent"]["name"] == ref["reviewer"]
                and r["data"]["round"] == ref["round"]
                and r["data"]["head"] == root["data"]["head"]
            ]
            if len(candidates) != 1 or not candidates[0]["ts"]:
                valid = False
                break
            r = candidates[0]
            findings = [
                f for f in r["data"]["findings"] if f["id"] == ref["finding_id"]
            ]
            if len(findings) != 1 or not findings[0]["location"]:
                valid = False
                break
            detected.append(r)
            locations.extend(
                re.findall(r"(?<![\w/])([\w./-]+):\d+", findings[0]["location"])
            )
        if not valid or not detected or not locations:
            continue
        detection_time = max(
            [
                r["ts"] if r["kind"] == "review" else r["data"]["finished_at"]
                for r in detected
            ],
            key=lambda value: datetime.fromisoformat(value.replace("Z", "+00:00")),
        )
        defect_head = root["data"]["head"]
        for ready in reviews:
            if (
                ready["data"]["verdict"] != "Ready"
                or not ready["ts"]
                or datetime.fromisoformat(ready["ts"].replace("Z", "+00:00"))
                >= datetime.fromisoformat(detection_time.replace("Z", "+00:00"))
            ):
                continue
            h = ready["data"]["head"]
            try:
                git(repo, "merge-base", "--is-ancestor", h, defect_head)
                for path in sorted(set(locations)):
                    if path.startswith("/") or ".." in path.split("/"):
                        raise ValueError("invalid cited repository path")
                    if git(repo, "rev-parse", h + ":" + path) != git(
                        repo, "rev-parse", defect_head + ":" + path
                    ):
                        break
                else:
                    result.append(
                        record(
                            "escape",
                            root["task"],
                            "reviewer",
                            ready["agent"],
                            ready["orchestrator"],
                            ready["sources"]
                            + root["sources"]
                            + [s for d in detected for s in d["sources"]],
                            dict(
                                ready_head=h,
                                defect_head=defect_head,
                                claim_event=root["id"],
                                locations=sorted(set(locations)),
                                detected_by=sorted(
                                    {
                                        d["data"]["kind"]
                                        if d["kind"] == "run"
                                        else "review"
                                        for d in detected
                                    }
                                ),
                                proof="unchanged-cited-files",
                            ),
                            detection_time,
                        )
                    )
            except (subprocess.CalledProcessError, ValueError, TypeError):
                continue
    return result
