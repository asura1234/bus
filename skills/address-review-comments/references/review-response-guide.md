# Review response guide

Separate three questions for every comment:

1. **Truth:** does current evidence support the reported problem?
2. **Scope:** is correcting it part of the locked goal and this owner's files?
3. **Remedy:** what is the smallest fix that preserves intended behavior?

A true observation does not automatically approve the reviewer's proposed fix.
An old valid comment can be `REJECT — already addressed` when current HEAD no
longer contains the defect. Conflicting recommendations should be surfaced with
their evidence; do not manufacture consensus.

Ledger fields: stable claim ID, source, normalized claim, current-HEAD evidence,
truth assessment, scope assessment, disposition, remedy, changed files, tests,
and residual risk.
