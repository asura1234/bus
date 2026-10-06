"""Rendering, parsing, and fail-closed validation of the gate-and-fix round artifact."""

from __future__ import annotations

import base64
import re
import shlex
from dataclasses import dataclass
from typing import Sequence


_LOG_ENCODING = "base64-utf8"
_SCHEMA_VERSION = "2"


@dataclass(frozen=True)
class GateResult:
    name: str
    argv: tuple[str, ...]
    exit_code: int | None
    duration_ms: int
    stdout: str
    stderr: str

    @property
    def passed(self) -> bool:
        return self.exit_code == 0


@dataclass(frozen=True)
class ArtifactGate:
    name: str
    passed: bool
    stdout: str
    stderr: str


def _render_log(title: str, value: str) -> list[str]:
    encoded = base64.b64encode(value.encode("utf-8")).decode("ascii")
    return [
        f"#### {title}",
        "",
        f"- Encoding: `{_LOG_ENCODING}`",
        f"- Bytes: `{len(value.encode('utf-8'))}`",
        "",
        "```base64",
        encoded,
        "```",
        "",
    ]


def render_round(
    *,
    round_number: int,
    base: str,
    head: str,
    changed_files: Sequence[str],
    results: Sequence[GateResult],
) -> str:
    """Produce the complete Markdown evidence defined by the format SOT."""
    outcome = "PASS" if all(result.passed for result in results) else "FAIL"
    lines = [
        "# Gate-and-Fix Round",
        "",
        f"- Schema: `{_SCHEMA_VERSION}`",
        f"- Round: `{round_number}`",
        f"- Base: `{base}`",
        f"- Head: `{head}`",
        f"- Outcome: `{outcome}`",
        "",
        "## Changed files",
        "",
        *[f"- `{path}`" for path in sorted(set(changed_files))],
        "",
        "## Gate results",
        "",
    ]
    for result in results:
        status = "PASS" if result.passed else "FAIL"
        exit_code = str(result.exit_code) if result.exit_code is not None else "launch-error"
        lines.extend(
            [
                f"### {result.name} — {status}",
                "",
                f"- Command: `{shlex.join(result.argv)}`",
                f"- Exit code: `{exit_code}`",
                f"- Duration: `{result.duration_ms}ms`",
                "",
                *_render_log("stdout", result.stdout),
                *_render_log("stderr", result.stderr),
            ]
        )
    return "\n".join(lines)


def _expect(lines: list[str], index: int, expected: str) -> int:
    if index >= len(lines) or lines[index] != expected:
        actual = "<eof>" if index >= len(lines) else repr(lines[index])
        raise ValueError(f"artifact structure error: expected {expected!r}, got {actual}")
    return index + 1


def _parse_log(lines: list[str], index: int, title: str, *, allow_eof: bool = False) -> tuple[int, str]:
    index = _expect(lines, index, f"#### {title}")
    index = _expect(lines, index, "")
    index = _expect(lines, index, f"- Encoding: `{_LOG_ENCODING}`")
    if index >= len(lines) or not re.fullmatch(r"- Bytes: `[0-9]+`", lines[index]):
        raise ValueError("artifact log Bytes is invalid")
    byte_count = int(lines[index].removeprefix("- Bytes: `").removesuffix("`"))
    index += 1
    index = _expect(lines, index, "")
    index = _expect(lines, index, "```base64")
    if index >= len(lines):
        raise ValueError("artifact log is missing its base64 content")
    encoded = lines[index]
    index += 1
    index = _expect(lines, index, "```")
    try:
        value = base64.b64decode(encoded, validate=True)
    except ValueError as error:
        raise ValueError("artifact log base64 is invalid") from error
    if len(value) != byte_count:
        raise ValueError("artifact log Bytes does not match its content")
    try:
        decoded = value.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError("artifact log is not UTF-8") from error
    if allow_eof and index == len(lines):
        return index, decoded
    return _expect(lines, index, ""), decoded


def _parse_round(artifact: str, *, expected_base: str) -> tuple[str, dict[str, ArtifactGate]]:
    """Parse the artifact fail-closed and return each gate's logs only after full validation."""
    lines = artifact.splitlines()
    index = 0
    index = _expect(lines, index, "# Gate-and-Fix Round")
    index = _expect(lines, index, "")
    index = _expect(lines, index, f"- Schema: `{_SCHEMA_VERSION}`")
    if index >= len(lines) or not re.fullmatch(r"- Round: `[1-9][0-9]*`", lines[index]):
        raise ValueError("artifact Round is invalid")
    index += 1
    index = _expect(lines, index, f"- Base: `{expected_base}`")
    if index >= len(lines) or not re.fullmatch(r"- Head: `[^`]+`", lines[index]):
        raise ValueError("artifact Head is invalid")
    index += 1
    if index >= len(lines) or not re.fullmatch(r"- Outcome: `(PASS|FAIL)`", lines[index]):
        raise ValueError("artifact Outcome is invalid")
    outcome = lines[index].removeprefix("- Outcome: `").removesuffix("`")
    index += 1
    index = _expect(lines, index, "")
    index = _expect(lines, index, "## Changed files")
    index = _expect(lines, index, "")
    changed_files = []
    while index < len(lines) and lines[index].startswith("- `"):
        match = re.fullmatch(r"- `([^`]+)`", lines[index])
        if match is None:
            raise ValueError("artifact Changed files entry is invalid")
        changed_files.append(match.group(1))
        index += 1
    if not changed_files or changed_files != sorted(set(changed_files)):
        raise ValueError("artifact Changed files must be nonempty, sorted, and unique")
    index = _expect(lines, index, "")
    index = _expect(lines, index, "## Gate results")
    index = _expect(lines, index, "")
    statuses: list[bool] = []
    gates: dict[str, ArtifactGate] = {}
    while index < len(lines):
        match = re.fullmatch(r"### ([a-z0-9-]+) — (PASS|FAIL)", lines[index])
        if match is None:
            raise ValueError("artifact Gate result heading is invalid")
        gate_name = match.group(1)
        if gate_name in gates:
            raise ValueError("artifact has a duplicate gate name")
        status = match.group(2)
        index += 1
        index = _expect(lines, index, "")
        if index >= len(lines) or not re.fullmatch(r"- Command: `[^`]+`", lines[index]):
            raise ValueError("artifact Command is invalid")
        index += 1
        if index >= len(lines) or not re.fullmatch(r"- Exit code: `(?:-?[0-9]+|launch-error)`", lines[index]):
            raise ValueError("artifact Exit code is invalid")
        exit_code = lines[index].removeprefix("- Exit code: `").removesuffix("`")
        passed = exit_code != "launch-error" and int(exit_code) == 0
        if (status == "PASS") != passed:
            raise ValueError("artifact gate status does not match its exit code")
        statuses.append(passed)
        index += 1
        if index >= len(lines) or not re.fullmatch(r"- Duration: `[0-9]+ms`", lines[index]):
            raise ValueError("artifact Duration is invalid")
        index += 1
        index = _expect(lines, index, "")
        index, stdout = _parse_log(lines, index, "stdout")
        index, stderr = _parse_log(lines, index, "stderr", allow_eof=True)
        gates[gate_name] = ArtifactGate(
            name=gate_name,
            passed=status == "PASS",
            stdout=stdout,
            stderr=stderr,
        )
    if not statuses:
        raise ValueError("artifact needs at least one gate result")
    derived_outcome = "PASS" if all(statuses) else "FAIL"
    if outcome != derived_outcome:
        raise ValueError("artifact Outcome does not match the gate results")
    return outcome, gates


def validate_round(artifact: str, *, expected_base: str) -> str:
    """Validate the artifact structure, result enums, and aggregate outcome fail-closed."""
    return _parse_round(artifact, expected_base=expected_base)[0]
