#!/usr/bin/env python3
"""Export surviving Bus records and receipt-linked sends from COPIED evidence.

Never executes transcript commands, opens a Bus socket, or changes an input.
Outputs are evidence for review, not state documents suitable for import.
"""
import argparse
import hashlib
import json
import re
import shlex
from pathlib import Path

RECEIVED = {22574: "Review round 1 for #148 has one proven finding."}
MESSAGE_IDS = {22193, 22223, 22235, 22287, 22461, 22628, 22574}
AGENTS = {21829, 17580, 18164, 18521, 18489, 18492, 16883}
NAMES = {"inv-p4", "cx-revisit", "cx-canvasbar", "inv-p2", "rev-claude8", "rnc-cx",
         "html-model-ui-2", "html-export-2", "html-preview-2", "html-preview", "rev-codex", "rev-claude"}


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)
    elif isinstance(value, dict):
        for key, item in value.items():
            if key not in {"encrypted_content", "image_url", "data"}:
                yield from strings(item)


def decoded_objects(value):
    """Unwrap provider tool results and nested CLI stdout, without executing text."""
    seen = set()
    pending = list(strings(value))
    for text in pending:
        if text in seen:
            continue
        seen.add(text)
        for candidate in [text, *text.splitlines()]:
            try:
                obj = json.loads(candidate)
            except (ValueError, TypeError):
                continue
            if isinstance(obj, dict):
                yield obj
                pending.extend(strings(obj))


def receipts(value):
    """Identify send receipts, including posted sends with no request IDs.

    Bare status responses have requests instead of message.send's request_ids.
    The CLI also preserves an explicit send_receipt while following --async,
    including when the follow-up fails after the original send was accepted.
    """
    for obj in decoded_objects(value):
        result = obj.get("result", {})
        if not isinstance(result, dict):
            continue
        receipt = result.get("send_receipt", result)
        if (isinstance(receipt, dict) and "message_id" in receipt
                and isinstance(receipt.get("request_ids"), list)
                and result.get("message_id") == receipt["message_id"]
                and (obj.get("ok") is True or "send_receipt" in result)):
            yield result


def commands(value):
    if isinstance(value, dict):
        if isinstance(value.get("command"), str):
            yield value["command"]
        if isinstance(value.get("cmd"), str):
            yield value["cmd"]
    elif isinstance(value, str):
        try:
            parsed = json.loads(value)
        except ValueError:
            parsed = None
        if isinstance(parsed, dict):
            yield from commands(parsed)
        # Codex exec calls contain JavaScript string literals. Decode JSON
        # double-quoted strings only; never eval JavaScript or run shell text.
        for match in re.finditer(r'\bcmd\s*:\s*("(?:[^"\\]|\\.)*")', value):
            yield json.loads(match[1])
        if " send " in value and "cmd:" not in value:
            yield value


def sends(command):
    # A send in a multiline Bash command is normally its own physical line.
    for line in command.replace("\\\n", " ").splitlines():
        if " send " not in line:
            continue
        try:
            words = shlex.split(line)
        except ValueError:
            continue
        for index, word in enumerate(words):
            if word != "send":
                continue
            options = {}
            for flag in ("--room", "--as", "--to", "--text", "--file"):
                if flag in words[index + 1:]:
                    at = words.index(flag, index + 1)
                    if at + 1 < len(words):
                        options[flag[2:]] = words[at + 1]
            if "text" in options or "file" in options:
                yield options


def relevant_send(send):
    tokens = {token for field in ("as", "to") for token in send.get(field, "").split(",")}
    if tokens.intersection(str(a) for a in AGENTS):
        return True
    names = tokens.intersection(NAMES)
    if send.get("room") == "8805":
        return any(n.startswith("html-") or n in {"rev-codex", "rev-claude"} for n in names)
    return send.get("room") == "8259" and bool(names) or send.get("room") == "877" and "rnc-cx" in names


def bus_records(value):
    if isinstance(value, dict):
        prompt = value.get("prompt")
        if isinstance(prompt, dict) and "recipient_ids" in prompt:
            yield value
            return
        if "recipient_ids" in value and "submitted_at_ms" in value:
            yield {"prompt": value}
            return
        for child in value.values():
            yield from bus_records(child)
    elif isinstance(value, list):
        for child in value:
            yield from bus_records(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence", type=Path, required=True, action="append")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    roots = [root.resolve(strict=True) for root in args.evidence]
    output = args.output.resolve()
    if any(output == root or output.is_relative_to(root) or root.is_relative_to(output) for root in roots):
        parser.error("output and copied evidence directories must be separate")
    # Refuse known live/provider roots even when accidentally passed by hand.
    forbidden = [Path.home() / p for p in (".local/share/bus", ".local/share/herdr", ".claude", ".codex")]
    if any(p == f or p.is_relative_to(f) for p in [output, *roots] for f in forbidden):
        parser.error("copy source files outside live/provider directories before running")
    output.mkdir(parents=True, exist_ok=True)
    exports = {}
    scanned = []

    def emit(kind, record, path, line, extra=None):
        payload = {"kind": kind, "record": record, **(extra or {})}
        key = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
        entry = exports.setdefault(key, {**payload, "sources": []})
        source = {"copy": str(path), "line": line}
        if source not in entry["sources"]:
            entry["sources"].append(source)

    for root in roots:
        for path in sorted(root.rglob("*")):
            if not path.is_file() or path.suffix not in {".json", ".jsonl"}:
                continue
            if path.name == "manifest.json":
                continue
            scanned.append(str(path))
            if "state.json" in path.name or "history.jsonl" in path.name:
                stream = [path.read_text()] if path.suffix == ".json" else path.open()
                for line, text in enumerate(stream, 1):
                    try:
                        doc = json.loads(text)
                    except ValueError:
                        continue
                    for record in bus_records(doc):
                        prompt = record["prompt"]
                        author = prompt.get("author", {})
                        author_id = author.get("agent") if isinstance(author, dict) else None
                        if (prompt.get("id") in MESSAGE_IDS or record.get("agent_id") in AGENTS
                                or author_id in AGENTS or AGENTS.intersection(prompt.get("recipient_ids", []))):
                            emit("bus_record", record, path, line)
                continue
            if path.suffix != ".jsonl":
                continue
            calls = {}
            for line, text in enumerate(path.open(), 1):
                # Ignore large API request snapshots, token counts and reasoning.
                try:
                    event = json.loads(text)
                except ValueError:
                    continue
                kind = event.get("type")
                p = event.get("payload", event)
                if kind == "response_item" and p.get("type") in {"custom_tool_call", "function_call"}:
                    calls[p.get("call_id")] = (p.get("input", p.get("arguments", "")), line)
                elif kind == "assistant":
                    for block in event.get("message", {}).get("content", []):
                        if isinstance(block, dict) and block.get("type") == "tool_use":
                            calls[block.get("id")] = (block.get("input", {}), line)
                if kind == "response_item" and p.get("type") == "message" and p.get("role") == "user":
                    for block in p.get("content", []):
                        body = block.get("text", "") if isinstance(block, dict) else ""
                        for message_id, prefix in RECEIVED.items():
                            if body.startswith("orchestrator (agent 878, room 0):\n" + prefix):
                                emit("correlated_received_payload", {"text": body}, path, line,
                                     {"correlated_message_id": message_id,
                                      "note": "ID inferred from matching room-history excerpt and recipient timeline; original receipt and request metadata not recovered."})
                results = []
                if kind == "response_item" and p.get("type") in {"custom_tool_call_output", "function_call_output"}:
                    results.append((p.get("call_id"), p.get("output")))
                elif kind == "user":
                    content = event.get("message", {}).get("content", [])
                    if isinstance(content, list):
                        results.extend((b.get("tool_use_id"), b.get("content")) for b in content
                                       if isinstance(b, dict) and b.get("type") == "tool_result")
                for call_id, result in results:
                    source, call_line = calls.get(call_id, (None, None))
                    sent = [s for cmd in commands(source) for s in sends(cmd)]
                    accepted = list(receipts(result))
                    for obj in decoded_objects(result):
                        for record in bus_records(obj):
                            prompt = record["prompt"]
                            author = prompt.get("author", {})
                            author_id = author.get("agent") if isinstance(author, dict) else None
                            if (prompt.get("id") in MESSAGE_IDS or record.get("agent_id") in AGENTS
                                    or author_id in AGENTS or AGENTS.intersection(prompt.get("recipient_ids", []))):
                                emit("embedded_bus_record", record, path, line)
                    if not accepted:
                        for send in sent:
                            if relevant_send(send):
                                emit("unconfirmed_send_command", send, path, call_line,
                                     {"note": "Authored command only; no receipt recovered. Does not prove Bus accepted it."})
                    for receipt in accepted:
                        if len(sent) == 1 and (receipt["message_id"] in MESSAGE_IDS or relevant_send(sent[0])):
                            emit("receipt_linked_send", sent[0], path, call_line,
                                 {"receipt": receipt, "receipt_line": line,
                                  "note": "Shell text parsed without execution; dynamic expansions are not reconstructed."})
    for key, entry in exports.items():
        (output / (key + ".json")).write_text(json.dumps(entry, ensure_ascii=False, indent=2) + "\n")
    summary = {"files_scanned": len(scanned), "exports": len(exports),
               "files": sorted(key + ".json" for key in exports),
               "message_ids": sorted({e.get("correlated_message_id", e.get("receipt", {}).get("message_id", e["record"].get("prompt", {}).get("id")))
                                      for e in exports.values()} - {None}),
               "kinds": {k: sum(e["kind"] == k for e in exports.values())
                         for k in {e["kind"] for e in exports.values()}}}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
