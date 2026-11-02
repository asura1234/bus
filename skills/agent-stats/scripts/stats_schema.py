"""Strict schema validation for local, source-linked work statistics."""

import hashlib
import json
import re
from datetime import datetime
from pathlib import Path

SCHEMA = json.loads(
    (Path(__file__).resolve().parents[1] / "references/stats.schema.json").read_text()
)


def canonical(value):
    return json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    )


def digest(value):
    return hashlib.sha256(canonical(value).encode()).hexdigest()


def pairs(items):
    result = {}
    for key, value in items:
        if key in result:
            raise ValueError(f"invalid duplicate JSON key: {key}")
        result[key] = value
    return result


def loads(text):
    return json.loads(
        text,
        object_pairs_hook=pairs,
        parse_constant=lambda x: (_ for _ in ()).throw(
            ValueError(f"invalid number: {x}")
        ),
    )


def read_json(path):
    return loads(Path(path).read_text(encoding="utf-8"))


def check(value, spec, where="$"):
    types = {
        "object": dict,
        "array": list,
        "string": str,
        "integer": int,
        "boolean": bool,
        "null": type(None),
    }
    allowed = spec.get("type", list(types))
    allowed = [allowed] if isinstance(allowed, str) else allowed
    if not any(type(value) is types[t] for t in allowed):
        raise ValueError(f"invalid {where}: expected {allowed}")
    if "enum" in spec and value not in spec["enum"]:
        raise ValueError(f"invalid {where}: unknown value {value!r}")
    if value is None:
        return
    if isinstance(value, dict):
        props = spec.get("properties", {})
        missing = set(spec.get("required", [])) - value.keys()
        extra = (
            value.keys() - props.keys()
            if spec.get("additionalProperties") is False
            else set()
        )
        if missing or extra:
            raise ValueError(
                f"invalid {where}: missing {sorted(missing)}, unknown {sorted(extra)}"
            )
        for key in value.keys() & props.keys():
            check(value[key], props[key], f"{where}.{key}")
    elif isinstance(value, list):
        if len(value) < spec.get("minItems", 0):
            raise ValueError(f"invalid {where}: too few items")
        for index, item in enumerate(value):
            check(item, spec.get("items", {}), f"{where}[{index}]")
    elif isinstance(value, str):
        if len(value) < spec.get("minLength", 0) or (
            "pattern" in spec and not re.search(spec["pattern"], value)
        ):
            raise ValueError(f"invalid {where}: bad string")
        if spec.get("format") == "date-time":
            if not re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", value):
                raise ValueError(f"invalid {where}: expected UTC timestamp")
            datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")
    elif type(value) is int and value < spec.get("minimum", value):
        raise ValueError(f"invalid {where}: below minimum")


def validate(record):
    check(record, SCHEMA)
    check(record["data"], SCHEMA["data_schemas"][record["kind"]], "$.data")
    payload = {k: v for k, v in record.items() if k != "id"}
    if record["id"] != digest(payload):
        raise ValueError("invalid event id: content hash mismatch")
    return record


def record(kind, task, role, agent, orchestrator, sources, data, ts=None):
    value = dict(
        schema_version=1,
        kind=kind,
        task=task,
        ts=ts,
        role=role,
        agent=agent,
        orchestrator=orchestrator,
        sources=sources,
        data=data,
    )
    value["id"] = digest(value)
    return validate(value)


def source(path, locator="$"):
    path = Path(path).resolve()
    return dict(
        path=str(path),
        sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
        locator=locator,
    )


def unknown(name="unknown"):
    return dict(id="unknown", name=name, provider="unknown", model="unknown")
