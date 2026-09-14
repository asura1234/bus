# {module-name}

> This file is the collaboration entrypoint for the module's current
> implementation. It is not a copy of source code, API reference, test output,
> or task progress. A sibling `CLAUDE.md`, when present, points here. When this
> document and code disagree, verify the as-built behavior from code and
> machine sources of truth, then reconcile this document.

## Module responsibility

Use one paragraph to state the sole owner, core responsibilities, and what the
module does not own. Record only facts that remain non-obvious after reading
the files and that affect change correctness.

## Dependencies

List first-party dependency directions and deliberately forbidden dependencies,
with reasons. Do not copy current third-party versions here; defer to
`Cargo.toml`, lockfiles, or the applicable build configuration.

Example:

- Depends on `crate::example_contract`: consumes contract types only.
- Must not depend on the client shell: the module remains UI-neutral.

## Stable boundaries and invariants (when needed)

Use this section only when the module owns a distinctive contract, such as:

- payloads allowed or forbidden across process or language boundaries;
- sender, generation, revision, capability, or path-authorization boundaries;
- the sole state owner and failure or cancellation semantics;
- where platform differences must converge.

Do not repeat a full API that is directly discoverable from types, public
definitions, or code structure. Use those sources or generated documentation
as the API reference.

## Validation and discovery (when needed)

- Use the repository-root `justfile` or the direct Cargo/Python/Bun equivalent
  when the wrapper is unavailable.
- List only module-specific gates that cannot be discovered naturally from the
  runner; do not duplicate full argument examples or per-test inventories.
- Obtain current test counts, file counts, and coverage results from the runner
  or report rather than storing them in this long-lived source of truth.

## Formal references (when needed)

Link only current architecture, protocol, contract, catalog, manifest, or lock
sources. Plans, working notes, issues, PRs, and one-time acceptance reports may
explain history, but must not become a second source required to understand the
current capability.

## Content forbidden from this file

- Literal current dependency or tool versions; link the machine source instead.
- Complete directory trees, per-export/per-test/per-log inventories, or current
  consumer and test counts.
- “Task N,” “this round,” “not rerun,” dated observations, incident timelines,
  migration progress, or debt dashboards.
- Personal absolute paths or mandatory skill calls that are absent from the
  current `Available skills` list.

Keep the body around 50–150 lines. When a complex cross-module protocol exceeds
that range, create a dedicated architecture or protocol source of truth and
retain only the module-specific conclusions and links here.
