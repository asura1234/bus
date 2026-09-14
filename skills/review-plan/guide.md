# Plan review guide

Review the plan's promised outcome, not an imagined replacement project.

Findings are deliberately unranked. Use stable IDs such as `F-01`, `F-02`, and
so on only to reference them across review rounds. The reviewer must not add or
imply severity or priority with labels, headings, fields, groups, or prose
judgments. Banned language includes `Blocker`, `Critical`, `Major`, `Minor`,
`P0` through `P3`, high/medium/low severity or priority, and equivalent ranking
terms.

Every reported finding is required work before `Ready`. If a claim is not
supported, actionable, and within the locked goal, omit it instead of assigning
it a lower rank. Pure nits are not findings.

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

### F-01 <short title>
- Claim: ...
- Evidence: `path:line` ...
- Consequence: ...
- Required correction: ...

## Resolved since prior round
- ...
```
