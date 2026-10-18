# Layered Skill Architecture

This document defines the information layering, single sources of truth, and intermediate-artifact protocol of Bus workflow skills. The goal is to make skills easy for agents to execute, make mechanical constraints verifiable, and keep the same rule from scattering across several natural-language documents and drifting continuously.

## Core principles

A complete workflow skill consists of four kinds of responsibility:

| Layer      | Medium                     | Owns                                                                                              | Does not own                                              |
| ---------- | -------------------------- | ------------------------------------------------------------------------------------------------- | --------------------------------------------------------- |
| Execution  | `SKILL.md`                 | executable pseudocode, command order, branches, loops, STOP conditions, when to read other SOTs   | long rationale, complete format templates, manual parsing |
| Principles | `guide.md` or shared guide | judgment principles, boundaries, priorities, risks, and exceptions                                | field order, fixed headings, mechanical validation        |
| Mechanics  | Python scripts             | parsing, normalization, validation, state computation, fail-closed gates, deterministic rendering | review conclusions that need semantic judgment, architecture trade-offs |
| Format     | `*-format.md`              | the strict structure, fields, enums, order, and empty-value expression of intermediate and final artifacts | workflow order, judgment principles, script implementation details |

A rule may have only one authoritative layer. Other layers may only reference it and must not copy an approximate restatement.

## 1. `SKILL.md`: executable pseudocode

`SKILL.md` is the agent's execution entrypoint and should read like runnable control flow:

- List reads, calls, branches, loops, waits, and termination conditions step by step.
- State explicitly when the guide and format must be read.
- Call scripts directly for mechanical steps; do not ask the agent to rewrite parsing logic on the spot.
- Keep only the short instructions execution requires; push the "why" down into the guide.
- Do not embed complete report templates; only reference the corresponding `*-format.md`.
- Do not rewrite script output by hand; when chat output must be fixed, return the renderer's product verbatim.

The `SKILL.md` of every canonical workflow skill listed in `scripts/skill_migration_contract_test.py` must stay within 250 lines, enforced by that test in `just maintenance-test`. The typical reason an entrypoint grows too long is not that the flow got more complex, but that guide principles, artifact templates, or pseudo-implementations of scripts flowed back into the entrypoint. The line-count gate only constrains entrypoint complexity; it does not prove the skill's quality — a 240-line `SKILL.md` that writes judgment principles as pseudocode passes just as well.

## 2. `guide.md`: guiding principles

The guide owns rules that the agent must understand and judge:

- Goals, non-goals, responsibility boundaries, and priorities.
- How to handle uncertainty, conflicts, exceptions, and reasonable deviation.
- When to escalate to the main agent or the developer.
- Why certain fail-open behaviors are unacceptable.
- Which checks are mechanical proofs and which still need semantic judgment.

The guide should not copy the complete steps of `SKILL.md`, nor maintain an artifact's headings, field order, or fixed template.
The execution entrypoint must explicitly require reading the corresponding guide completely before the relevant judgment happens; it cannot assume the agent will discover it on its own.

## 3. Python scripts: mechanical checks and gates

Work that is low-discretion, prone to drift, or needs repeated execution must be pushed down into Python:

- Fence-aware / section-aware parsing.
- Path, task, lane, hash, and state identity computation.
- Schema and enum validation.
- Fail-closed prerequisite gates.
- Deterministic trimming, aggregation, and rendering.
- Stable stdout, JSON, or exit code contracts.

Scripts must:

- Return non-zero for malformed, missing, duplicate, out-of-order, and unknown enum values.
- Not fill in missing fields by guessing.
- Produce the same output for the same input.
- Provide automated tests for the normal path, boundary paths, and fail-closed paths.
- Use the same fields and enums as the format SOT, and not maintain a second implicit schema.

Python can prove structure and state transitions; it cannot prove that the model really completed the semantic review. The latter relies on reviewer re-verification, cross-model dogfooding, and final human judgment.

## 4. `*-format.md`: strict artifact formats

Every artifact that is consumed across agents, rounds, or skills must have an independent format SOT. File names use a concrete domain prefix, for example:

- `review-format.md`
- `dead-code-findings-format.md`
- `gate-round-format.md`

The format document must pin down:

- The fixed order of the title and sections.
- Required, optional, and forbidden fields.
- Legal enums and casing.
- The single expression for an empty set, no issues, and not applicable.
- The normalized form of fields such as paths, ids, hashes, and times.
- Content that must or must not appear in different states.
- A complete positive example, plus explanations of malformed counterexamples where needed.

Formats shared by several skills live in `docs/guides/`; a format used by only one skill may live in that skill's directory (`references/`). Never copy the complete template again into `SKILL.md`, a guide, or script comments.

## Intermediate artifact lifecycle

The standard artifact flow is:

```text
agent writes the artifact per *-format.md
  → Python parser / gate validates fail-closed
  → Python renderer produces a compact completion envelope
  → producer agent returns the envelope verbatim
  → consumer agent proceeds per the SKILL.md state machine
```

Complete evidence stays in the artifact file; messages between agents carry only status, summary, and artifact path. This both reduces pollution of the main agent's context and avoids fields being lost through the model's hand-written summaries.

If an artifact fails mechanical validation:

- The producer must not report completion.
- The consumer must not guess the status from natural language.
- The producer fixes the artifact and reruns the gate.

## Communication between agents

Communication between agents must distinguish two kinds of information:

- **Control messages**: short; say what happened and what decision the main agent now has to make.
- **Evidence artifacts**: complete and persisted, for later acceptance, resume, and audit.

A control message contains at least the task identity, the conclusion, and the artifact path. The conclusion must be explicit enough that the main agent can act on it directly; free text such as "basically done" or "should work" must not stand in for it. Detailed touched files, gate output, and concerns are written into the artifact, not pasted into the control message.

Control messages **do not need** a state machine of their own: action envelopes, generation hashes, mailboxes, and completion-file polling are all scaffolding an agent asks for to confirm "which step am I on", not defenses bought by an observed failure. Orchestration is held by the main agent itself; scripts keep only the part it cannot prove on its own.

A subagent must report immediately when it reaches a state that needs the main agent to act, rather than waiting for a hypothetical "final completion":

- Missing context.
- Owner gap.
- A blocker it cannot get past.
- Completed but with correctness concerns.

The main agent waits only on agents known to be still running; when it has local orchestration or acceptance work, it does that first. Waiting should use the longest safe window the host allows and handle any agent's report immediately; no continuous short-interval busy-polling, and no repeatedly asking "are you done?".

## Directories and references

Recommended structure:

```text
skills/<skill>/
├── SKILL.md
├── guide.md
├── references/
│   └── <artifact>-format.md
└── scripts/
    └── <mechanical-task>.py

docs/guides/
└── <artifact>-format.md

docs/templates/
└── <shared-template>.md

cli_extensions/
└── <shared-mechanical-helper>.py

.agents/skills/<skill> -> ../../skills/<skill>
.claude/skills/<skill> -> ../../skills/<skill>
```

`skills/` is the only editable skill root; `.agents/skills` and `.claude/skills` are discovery links. `SKILL.md` must link directly to the guides and formats this run needs; do not form deep guide → reference → reference chains. Scripts are invoked by command; unless a script is being modified, the agent does not need to read its source first.

## Change checklist

When modifying or adding a workflow skill:

1. Decide whether the new rule belongs to the execution, principle, mechanics, or format layer.
2. Modify only that rule's authoritative SOT; other layers only add references or calls.
3. When adding or modifying an artifact, update `*-format.md` first, then the parser / renderer.
4. Add normal and fail-closed tests for mechanical contracts.
5. Confirm `SKILL.md` stays within 250 lines (`just maintenance-test` verifies it).
6. Run the applicable `just lint`, the affected Python tests / `just maintenance-test`, and the skill symlink checks.
7. Dogfood with a real task; only solidify repeated failures or high-risk fail-open behavior into a new control surface.

## Forbidden patterns

- Piling guiding principles, format templates, and script pseudo-implementations all into `SKILL.md`.
- The same field or enum written separately in several documents.
- A format claiming to be strict while the parser accepts missing sections, wrong order, or unknown states.
- The parser producing one structure and the renderer interpreting it by another set of rules.
- Letting the agent hand-summarize or rewrite output that could be generated deterministically.
- Declaring completion merely because the artifact file exists, without running the mechanical gate.
- Adding a new state machine, metric, or second source of truth for a single low-impact preference feedback.
