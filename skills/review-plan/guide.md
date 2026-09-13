# Plan review guide

Review the plan's promised outcome, not an imagined replacement project.

Severity:

- **Blocker**: unsafe, impossible, or incapable of meeting the goal.
- **Major**: likely correctness, lifecycle, migration, or verification gap.
- **Minor**: bounded ambiguity that can cause avoidable rework.

Strong findings cite current repository evidence and explain a concrete failure
path. Do not demand exhaustive testing, stylistic rewrites, or code that is
outside the locked goal. A follow-up is closed-world: prior findings, the new
delta, and consequences introduced by that delta.

Suggested artifact:

```markdown
# Plan review: <plan>

- Reviewer: <lane>
- Round: <N>
- Verdict: Ready | Not Ready
- Reviewed commit: <SHA>

## Findings

### [Major] <short title>
- Claim: ...
- Evidence: `path:line` ...
- Consequence: ...
- Required correction: ...

## Resolved since prior round
- ...
```
