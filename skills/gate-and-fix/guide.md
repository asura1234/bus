# Gate and fix guide

Bus is primarily Rust but includes Python maintenance tooling, Bun-based assets,
and release documentation. Gate selection follows the changed surface rather
than a copied one-size-fits-all command.

`just ci` is the repository's full pre-PR gate. Use focused commands during the
repair loop so failures remain attributable, then run the full gate once the
tree is composed. Never import LibTV Desktop's `./run`, pnpm workspace, Electron
session, coverage-partition, or submodule procedures.

The host may not have `just` or `cargo-nextest`. Inspect the `justfile` and use
direct commands for focused iteration rather than guessing. A `cargo test`
fallback is useful evidence but does not satisfy a policy that specifically
requires `just ci`; disclose and resolve that provisioning gap before PR.

A passing command is evidence only for what it exercised. Record unavailable or
skipped checks plainly. Existing unrelated failures are not permission to
change unrelated files; prove they predate the owned diff and report them.
