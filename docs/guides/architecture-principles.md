# Architecture Principles

This document is the architecture judgment baseline shared by code review and plan review. The [code review guide](code-review-guide.md) and
the [plan review guide](plan-review-guide.md) cite this document rather than each maintaining another set of principles.

This document specifies only stable judgment criteria; it does not maintain package, API, schema, or lint inventories, or large drift-prone
implementation examples. Module dependencies and invariants follow the code and its tests, and concrete as-built evidence is taken from code,
contracts, configuration, and tests.

## Layering and boundaries

### Separation of concerns

Divide business logic, state orchestration, views, and infrastructure by responsibility:

- Business logic expresses domain rules and computation, does not depend on a UI framework or platform API, and can be verified independently.
- The state layer owns state and interaction orchestration and does not carry reusable domain rules.
- Views render and forward interaction and do not establish a second set of business state.
- Infrastructure wraps external capabilities such as platform, storage, network, and rendering, and offers them to upper layers through stable boundaries.

Tests may combine real dependencies across layers; production code's ownership and dependency direction must not be mixed because of that. New
logic should go to its single Owner, not to the place closest to the current call site.

### Single responsibility

Functions, files, and modules each revolve around one directly describable theme. When there are multiple independent reasons to change, the
main flow is drowned by many side paths, or unrelated state and lifecycles are owned at the same time, split along stable responsibilities; do
not cut mechanically by line count.

### Minimal public API (visibility)

Expose only the symbols that real cross-boundary consumers need. Internal helpers, constants, intermediate carriers, and implementation types
stay private; when adding or widening a public surface, confirm consumers and boundaries with a code search. A public API is a long-term
contract and must not be widened for export convenience or imagined reuse.

Each language's "public" mechanism differs; the principle is the same:

- **Rust**: items are private to their module by default; tighten cross-module access with `pub(super)` / `pub(crate)`, and use `pub` only for surfaces consumed outside the crate or that form a command/protocol contract. Re-export from `mod.rs` / `lib.rs` only the symbols really used across modules.

### Dependency direction

Higher layers define the contracts they need, lower layers provide implementations; the Composition Root does the assembly.

Cross-module code depends only on the public surface, does not reach into another module's internals, and forms no reverse dependency or
cycle. Views may depend on state, and state may depend on business logic; business logic does not depend back on state or views. Platform
differences converge inside platform implementations or adapters.

### When dependency injection applies

Use dependency injection when a dependency must share one instance, swap a platform implementation, or be assembled by an explicit lifecycle
Owner. A dependency with a single call site and no implementation variant is simply passed through a constructor or parameter. Injection must
not blur object ownership, nor become a reason to pre-install extension points.

### Interface segregation

Interfaces are designed around concrete usage scenarios; consumers should not depend on capabilities they do not use. An oversized interface
should be split along real responsibilities, but do not manufacture worthless abstractions with a single producer and consumer for the sake of
formal "segregation".

### Communicate through data flow, do not expose internal state

Pass read-only snapshots, explicit events, or narrow interfaces across modules; do not expose mutable internal objects and general-purpose
operation entry points. State is established and updated by a single Owner, and consumers observe it according to the contract without
maintaining parallel copies.

### Extension and modification

Stable boundaries should allow adding real variants without breaking unrelated consumers; but "open for extension" does not authorize building
frameworks, configuration, or abstractions for an unknown future. If modifying an existing union type or signature is exactly the real boundary
of the current requirement, modify the single Owner and all consumers in sync directly.

## Failure handling

### Validation belongs to the boundary Owner

External input is validated at the boundary where it enters the trusted domain and returns an explicit error; the same rule should not be
implemented repeatedly by multiple callers. Internal preconditions and invariants are guaranteed by the nearest state Owner; callers do not
mask the establishing party's defect with default values, extra optional types, or repeated checks.

### Fail-fast: invariant violations terminate at the nearest state Owner

An impossible state represents a code defect and should fail, always in effect, at the nearest Owner while preserving the original cause. Type
guards, contract validation, and exhaustiveness checks can make illegal states unrepresentable; assertions effective only in debug builds
(such as Rust's `debug_assert!`) cannot carry a production contract.

Rust uses the module's existing production-time check mechanism.

Externally expected failures — for example network, file, permission, or user-input errors — return failure per the contract and record the
necessary context. Do not wrongly turn this kind of failure into a process crash, and do not catch an internal defect and keep running. Logs go
through `tracing`, as configured in `src/logging.rs`.

### Boundaries of defensive fallback

Fallback is used only for real, permitted, observable, and testable degradation. In the following cases, do not add fallback, retries,
placeholder values, or silent no-ops, and remove existing superfluous paths as well:

- after the dependency fails, the feature or the app has no meaningful way to continue;
- the state violates an established invariant;
- the candidate branch has no reachable input or product semantics supporting it.

Real recoverable failures must still have explicit errors, logs, and recovery semantics. This principle requires distinguishing failure
categories, not reducing necessary error handling.

## Complexity

### KISS: keep it simple, reject over-engineering

Abstractions, indirection layers, configuration items, and complex return structures all carry the burden of proof. They hold only when they
express a current real boundary, invariant, or reuse, and actually reduce overall complexity. Intermediate structures a local flow needs stay
local and are not promoted to shared contracts.

### DRY: one Owner per semantics

Logic with the same purpose, rules, and reason to change is implemented by one Owner and reused elsewhere. Code that looks similar but differs
in semantics or reason to change is not forcibly merged; when an existing narrow capability needs to serve a new consumer with the same
semantics, prefer generalizing the original Owner over keeping an "old version" and a "generic version" side by side.

### YAGNI: do not pre-install for an unknown future

Extension points, configuration, state, compatibility branches, and public types not supported by a current reachable requirement must not
enter the implementation. "Might be needed in the future" alone cannot prove that a mechanism needs to exist; choose the abstraction under the
constraints of the time once the requirement actually appears.

### Minimal change surface

Bound the change by the minimal set of responsibilities needed to achieve the current goal. Do not smuggle in unrelated refactors, and do not
stuff logic that belongs to different Owners into one place to reduce file count. The change surface is measured by responsibility and risk,
not ruled by a fixed number of files or lines.

## Change discipline

### Follow existing patterns

First discover the current pattern from adjacent code, its callers and tests, and machine configuration. A new pattern is introduced only
when existing patterns cannot meet a real requirement, with its boundary and Owner stated; no existing pattern is exempt from DRY, YAGNI,
KISS, and the minimal public surface.

### Do it right once

For requirements that are already clear and within the current scope, deliver a complete, verifiable solution; do not cover known gaps with
"do it later". YAGNI targets the unknown future, while this principle targets clear current requirements; the two do not conflict.

Truly independent work may be split into single-purpose increments; each increment should close within its own boundary, without letting old
and new Owners or two sets of state coexist long term.

### Complete refactors at closable boundaries

Prefer completing a refactor in one independently verifiable change. When the scope is too large, narrow or split it along stable
responsibilities, keeping a single Owner and a runnable state at every step, and do not treat transition layers, dual writes, or temporary
compatibility as permanent architecture.

### No meaningless deprecation periods

For internal contracts delivered atomically in the same build, whose callers cannot upgrade independently and which do not cross a persistence
boundary, modify both sides in sync directly, without keeping `#[deprecated]`, dual reads, or old and new implementations. The boundary is
where versions can be skewed: a running server and a newer or older client across the client/server wire protocol (`PROTOCOL_VERSION`),
persisted session state, and external protocols must argue compatibility according to their version contract, and cannot apply the one-shot
migration rule.
