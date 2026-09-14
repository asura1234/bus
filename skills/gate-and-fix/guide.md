# Gate and fix guide

Bus is primarily Rust but includes Python maintenance tooling, Bun-based assets,
and release documentation. Gate selection follows the changed surface rather
than a copied one-size-fits-all command.

`just ci` is the repository's full pre-PR gate. Use focused commands during the
repair loop so failures remain attributable, then run the full gate once the
tree is composed. Never import LibTV Desktop's `./run`, pnpm workspace, Electron
session, coverage-partition, or submodule procedures.

The host may not have `just` or `cargo-nextest`. The deterministic runner
expands `just ci` into the corresponding direct Cargo, Python, and Bun gates on
that host. This is an explicit Bus adapter, and the round artifact records the
exact commands rather than claiming that an unavailable wrapper ran.

A passing command is evidence only for what it exercised. Record unavailable or
skipped checks plainly. Existing unrelated failures are not permission to
change unrelated files; prove they predate the owned diff and report them.
