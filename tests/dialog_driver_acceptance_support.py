"""Private acceptance route evidence; never supplies an answer to a prompt."""
import gzip
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def audit_records(root):
    return [json.loads(line) for file in sorted(root.rglob("fake-dialog-results.jsonl"))
            for line in file.read_bytes().splitlines(keepends=True)
            if line.endswith(b"\n") and line.strip()]


def append_record(path, record):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(descriptor, (json.dumps(record) + "\n").encode())
    finally:
        os.close(descriptor)


def trace_command(binary, data, trace, args):
    method = ".".join(args[:2]) if args and args[0] in {"agent", "pane", "api"} else (args[0] if args else "launch")
    record = {"method": method, "target": args[2] if len(args) > 2 and args[0] == "agent" else None}
    if method != "agent.press":
        append_record(trace, record)
        os.execv(binary, [binary, *args])
    record["audit_before"] = [row["id"] for row in audit_records(Path(data))]
    result = subprocess.run([binary, *args], capture_output=True)
    try:
        envelope = json.loads(result.stdout)
        press = envelope.get("result") or {}
        record.update(request_id=envelope.get("id"), ok=bool(envelope.get("ok") and result.returncode == 0),
                      terminal_id=press.get("observation", {}).get("terminal_id"),
                      outcome=press.get("outcome"), selection_pending=press.get("selection_pending"),
                      key_count=len(press.get("keys", [])), text_written=press.get("text_written", False))
    except (ValueError, TypeError, AttributeError):
        record["ok"] = False
    try:
        append_record(trace, record)
    finally:
        # The caller must see the original receipt, including uncertain outcomes.
        sys.stdout.buffer.write(result.stdout)
        sys.stderr.buffer.write(result.stderr)
    return result.returncode


def collect_logs(data, evidence):
    traces = []
    for role in ["client", "server"]:
        for index, source in enumerate(sorted(data.rglob(role + ".jsonl"))):
            shutil.copy2(source, evidence / f"{role}-{index}.jsonl")
        sources = sorted(p for p in data.glob(f"herdr-{role}.log*") if p.is_file()
                         and not any(part in p.name for part in [".lock", ".policy", ".tmp"]))
        for index, source in enumerate(sources):
            target = evidence / f"{role}-trace-{index}{'.gz' if source.suffix == '.gz' else '.log'}"
            shutil.copy2(source, target)
            traces.append(target)
    return traces


def route_records(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_bytes().splitlines(keepends=True)
            if line.endswith(b"\n") and line.strip()]


FIELDS = re.compile(r'([a-zA-Z_]+)=(?:"([^"]*)"|([^\s{}]+))')
FORBIDDEN = {"agent.send-keys", "agent.send_keys", "agent.choose", "agent.answer", "pane.send_input", "pane.send-input"}


def driver_route_proof(traces, records, expected, fake_terminal, shell_terminal, setup_ids):
    events = {}
    alternate = []
    for path in traces:
        content = gzip.decompress(path.read_bytes()) if path.suffix == ".gz" else path.read_bytes()
        for line in content.decode(errors="replace").splitlines():
            pairs = [(name, quoted or bare) for name, quoted, bare in FIELDS.findall(line)]
            fields = dict(pairs)
            if fields.get("event") == "bus.terminal.driver.result":
                events[fields.get("control_request_id")] = fields
            methods = {value for name, value in pairs if name == "method"}
            if methods & FORBIDDEN and fields.get("request_id", fields.get("api_request_id")) not in setup_ids:
                alternate.append({"methods": sorted(methods & FORBIDDEN), "request_id": fields.get("control_request_id", fields.get("request_id"))})
    alternate += [{"methods": [row["method"]]} for row in records if row["method"] in FORBIDDEN]

    def verified(row):
        event = events.get(row.get("request_id"), {})
        return (row.get("method") == "agent.press" and row.get("ok")
                and not row.get("selection_pending") and row.get("outcome") in {"sent", "selected"}
                and (row.get("key_count", 0) > 0 or row.get("text_written"))
                and event.get("terminal_id") == row.get("terminal_id")
                and event.get("outcome") == row.get("outcome")
                and event.get("key_count") == str(row.get("key_count"))
                and event.get("text_written") == str(row.get("text_written", False)).lower())

    inputs = [row for row in records if verified(row)]
    dialogs = {}
    for index, prompt in enumerate(expected):
        matches = [row for row in inputs if row.get("terminal_id") == fake_terminal
                   and row.get("audit_before") == expected[:index]]
        if matches:
            dialogs[prompt] = matches[-1]["request_id"]
    shell = [row["request_id"] for row in inputs if row.get("terminal_id") == shell_terminal]
    missing = [prompt for prompt in expected if prompt not in dialogs]
    return {"passed": not missing and bool(shell) and not alternate, "dialogs": dialogs,
            "shell_requests": shell, "missing_dialogs": missing, "alternate_input_methods": alternate,
            "native_driver_results": len(events), "setup_input_requests": sorted(setup_ids)}


if __name__ == "__main__":
    raise SystemExit(trace_command(*sys.argv[1:4], sys.argv[4:]))
