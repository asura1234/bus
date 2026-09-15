# Layered Skill Architecture

This document defines the information layers, sources of truth, and artifact protocols for Bus workflow skills. The goal is to make workflows easy for agents to execute, mechanical constraints verifiable, and rules resistant to drift.

## Core model

A complete workflow skill has four responsibility layers:

| Layer | Medium | Owns | Does not own |
| --- | --- | --- | --- |
| Execution | `SKILL.md` | executable pseudocode, command order, branches, loops, stop conditions, and required reads | long rationale, full templates, hand-written parsing |
| Principles | `guide.md` or shared guide | judgment rules, boundaries, priorities, risks, and exceptions | field order, fixed headings, mechanical validation |
| Mechanics | Python scripts | parsing, normalization, identity, state, fail-closed gates, and deterministic rendering | semantic review conclusions or architecture choices |
| Format | `*-format.md` | exact artifact structure, fields, enums, order, and empty values | workflow order, judgment principles, implementation |

One rule has one authoritative layer. Other layers may reference it but must not maintain an approximate duplicate.

## 1. `SKILL.md`: executable pseudocode

The entrypoint reads like executable control flow:

- list reads, calls, branches, loops, waits, and termination conditions;
- say exactly when guides and formats must be read;
- call deterministic helpers instead of recreating parsers in prose;
- keep only execution-critical instruction and move rationale to a guide;
- reference complete report templates rather than embedding them;
- return renderer output verbatim when chat output is fixed.

Keep entrypoints under 250 lines. Bus does not yet have LibTV Desktop's automatic skill-length lint, so this is an architectural constraint checked by the skill migration tests and review rather than a claim that `just lint` enforces it.

## 2. `guide.md`: judgment principles

A guide owns rules the agent must understand:

- goals, non-goals, responsibility boundaries, and priority;
- uncertainty, conflict, exceptions, and reasonable deviation;
- when to escalate to the main agent or developer;
- why a fail-open behavior is unsafe;
- which checks are mechanical proofs and which require semantic judgment.

Do not duplicate the complete `SKILL.md` flow or artifact headings and field order. The entrypoint must require the guide before the relevant judgment point.

## 3. Python scripts: mechanical checks and gates

Low-discretion, drift-prone, or repeated work belongs in Python:

- fence-aware and section-aware parsing;
- path, task, lane, hash, and state identities;
- schema and enum validation;
- fail-closed prerequisites;
- deterministic trimming, aggregation, and rendering;
- stable stdout, JSON, and exit-code contracts.

A mechanical helper must:

- reject malformed, missing, duplicate, out-of-order, and unknown values;
- never guess a missing field;
- produce the same output for the same input;
- test normal, boundary, and fail-closed paths;
- use the same field names and enums as the format source of truth.

Python can prove structure and state transitions. It cannot prove that a model performed a complete semantic review; reviewer verification, cross-model pressure testing, and developer judgment cover that boundary.

## 4. `*-format.md`: strict artifact formats

Every artifact consumed across agents, rounds, or skills has a separate format source of truth, such as:

- `review-format.md`
- `task-agent-report-format.md`
- `execute-plan-action-format.md`

The format fixes:

- headings and section order;
- required, optional, and forbidden fields;
- legal enums and casing;
- one representation for empty or not-applicable values;
- normalized paths, ids, hashes, and times;
- state-dependent required and forbidden content;
- a complete positive example and necessary malformed examples.

Shared guides and formats live under `docs/guides/`. A single-skill format may live under that skill's `references/`. Never duplicate a complete template inside an entrypoint, guide, or script comment.

## Artifact lifecycle

```text
agent writes artifact from *-format.md
  -> Python parser/gate validates fail-closed
  -> Python renderer creates a compact completion envelope
  -> producer returns the envelope verbatim
  -> consumer advances the SKILL.md state machine
```

Complete evidence remains in the artifact. Agent messages carry only status, identity, summary, and artifact path. This reduces orchestrator context pressure and prevents fields from disappearing through hand summaries.

If mechanical validation fails, the producer is not complete and the consumer cannot infer a state from prose. Correct the artifact and rerun the gate.

## Agent communication

Distinguish:

- **Control messages:** short structured state-machine inputs.
- **Evidence artifacts:** complete durable evidence for acceptance, resume, and audit.

A control message contains at least the closed status, task identity, and artifact path. Never substitute phrases such as "mostly done" or "should work" for a format enum. Touched files, gate output, and concerns live in the artifact.

A worker immediately reports missing context, owner gaps, blockers, or completed work with correctness concerns. The main agent waits only on known-running workers and progresses local orchestration first. Use bounded long waits, not repeated short polling or "are you done" messages.

## Room assignment context

Canonical workflow skills can run inside a Bus room. Each layer keeps its own authority:

- **Format:** `docs/guides/room-brief-projection-format.md` owns the trusted assignment projection and the verifier result.
- **Principles:** `docs/guides/orchestrated-room-brief.md` owns branch mapping, Goal/Non-goals precedence, and participant handoff.
- **Mechanics:** `cli_extensions/room_assignment_context.py` maps discovery and verifier results to one context; `skills/pr/scripts/pr_goal_context.py` produces the review Goal/Non-goals locks; `scripts/skill_goal_ownership_check.py` verifies the consuming skills.
- **Execution:** each consuming `SKILL.md` runs the shared consumer before deriving intent and branches on `verified` or `NotInBusRoom`; any other result stops with blocker evidence.

## Directory and discovery contract

```text
skills/<skill>/
├── SKILL.md
├── guide.md
├── references/
│   └── <artifact>-format.md
└── scripts/
    └── <mechanical-task>.py

docs/guides/
└── <shared-guide-or-format>.md

docs/templates/
└── <shared-template>.md

cli_extensions/
└── <shared-mechanical-helper>.py

.agents/skills/<skill> -> ../../skills/<skill>
.claude/skills/<skill> -> ../../skills/<skill>
```

`skills/` is the sole editable skill root. `.agents/skills` and `.claude/skills` are discovery links. An entrypoint directly links every guide and format needed for that run; avoid deep guide-to-reference chains. Scripts are invoked by command and do not need to be read unless they are being modified.

## Change checklist

1. Classify each rule as execution, principle, mechanic, or format.
2. Change only its authoritative source; other layers add only references or calls.
3. Change an artifact format before changing its parser or renderer.
4. Add normal and fail-closed tests for mechanical contracts.
5. Keep each `SKILL.md` under 250 lines.
6. Run affected Python tests, registry-link checks, `just lint`, and relevant Rust or maintenance tests.
7. Pressure-test the workflow with a realistic task; add control surface only for repeat failures or dangerous fail-open behavior.

## Forbidden patterns

- putting principles, full templates, and script pseudocode into one `SKILL.md`;
- defining one field or enum independently in several documents;
- claiming a strict format while its parser accepts missing, reordered, or unknown values;
- parsing one structure and rendering another;
- hand-summarizing deterministic output;
- treating artifact existence as completion without validation;
- adding a state machine, metric, or second source of truth for one low-impact preference.
