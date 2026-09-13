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

Use priorities sparingly:

- **P0**: catastrophic and immediate.
- **P1**: blocks landing; common or severe correctness/safety failure.
- **P2**: material but bounded defect.
- **P3**: worthwhile low-risk correction; omit pure nits.

Every finding must identify a concrete failure scenario supported by current
code. Do not assume a name match proves ownership, or that green tests cover an
unexercised path. Follow-up review is closed-world and may overturn an old claim
when current evidence disproves it.
