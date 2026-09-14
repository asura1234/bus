# Code review guide

Prioritize defects that can affect users or repository safety:

1. Goal and non-goal compliance.
2. Correctness on reachable paths and boundaries.
3. Security, authority, and untrusted input handling.
4. State ownership, persistence, cancellation, and concurrency.
5. Compatibility across supported operating systems and existing data.
6. Performance on hot paths.
7. Regression-test quality.
8. Focus and reviewability of the diff.

Findings are deliberately unranked. Use stable IDs such as `F-01`, `F-02`, and
so on only to reference them across review rounds. The reviewer must not add or
imply severity or priority with labels, headings, fields, groups, or prose
judgments. Banned language includes `Blocker`, `Critical`, `Major`, `Minor`,
`P0` through `P3`, high/medium/low severity or priority, and equivalent ranking
terms.

Every reported finding is required work before `Ready`. If a claim is not
supported, actionable, material, and within scope, omit it instead of assigning
it a lower rank. Pure nits are not findings.

Every finding must identify a concrete failure scenario supported by current
code. Do not assume a name match proves ownership, or that green tests cover an
unexercised path. Follow-up review is closed-world and may overturn an old claim
when current evidence disproves it.
