# Dead-code judgment guide

Dead means unowned by every supported production path, not merely absent from a
text search. Tests alone do not create production ownership, but a compatibility
fixture, protocol sample, migration, or integration asset may intentionally
preserve behavior.

Duplicate-looking implementations may encode platform, terminal, provider, or
failure-mode differences. Consolidate only after proving equivalent contracts.
Prefer a compiler-supported removal experiment plus focused tests over
speculation. When dynamic discovery or external consumers cannot be ruled out,
classify the candidate as `FLAG` rather than deleting it.
