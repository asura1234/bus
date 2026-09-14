# Documentation Audit Guide

Start from changed behavior, not from filenames alone. A documentation target is current only when a reader can still derive the implemented contract, ownership boundary, supported command, and failure behavior from it.

For `skills/AGENTS.md`, reconcile the canonical skill root, shared guide and helper locations, repository adapters, validation commands, and registry-link contract. Do not copy Desktop-specific `./run`, pnpm, Electron, submodule, or module-document rules into Bus.

For vendored `AGENTS.md`, preserve upstream scope and terminology. Do not rewrite vendored guidance merely because first-party Bus code changed elsewhere.

For public and release documentation, inspect direct consequences of user-visible CLI, configuration, socket, integration, install, and terminal behavior. Preserve the repository's English/translation parity rules. A source change that does not alter a documented contract should leave docs untouched and be recorded as verified current.

Mechanical target discovery cannot prove semantic accuracy. Read the changed code and the document together. If evidence is incomplete, leave the audit pending rather than guessing.
