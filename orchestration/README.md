# Orchestration contract

`orchestration/` and `workflows/` hold the default instructions for MASTER
agents. Rust embeds them with `include_str!`; debug, release and Nix builds
all carry those compiled-in copies. Updating embedded defaults requires rebuilding
Bus. Bus does not load prompt or rule defaults from the checkout, an installed
`share/bus` directory or a user override directory at runtime. A dev build or
`--dev` run with a detected checkout points workflow lookup to its absolute
`workflows/` directory; other launches point to the complete embedded session copy.

Work-room agents receive their ordinary work messages. MASTER agents receive
the orchestrator prompt for the work room they orchestrate, plus access to the
Bus docs written under `<BUS_DATA_DIR>/docs/`.

## Source files and emitted names

The generated docs retain their established names so every prompt and guide
refers to the same files. These are files written from the binary's embedded
content, not another editable source layer.

| Repository source | File written by Bus |
|---|---|
| `orchestration/prompt.md` | `<launch callback folder>/system-prompt.md`, after filling placeholders |
| `orchestration/how-to-bus-cli.md` | `<BUS_DATA_DIR>/docs/how-to-bus-cli.md` |
| `orchestration/guide.md` | `<BUS_DATA_DIR>/docs/orchestrator-guide.md` |
| `orchestration/rules.md` | `<BUS_DATA_DIR>/docs/orchestrator-rules.md`, with the workflow directory filled in |
| `workflows/create.md` | `<BUS_DATA_DIR>/docs/workflow-create.md` |
| `workflows/template.md` | `<BUS_DATA_DIR>/docs/templates/workflow-template.md` |
| `workflows/investigation-report-template.md` | `<BUS_DATA_DIR>/docs/templates/investigation-report.md` |
| `workflows/bug-triage-table-template.md` | `<BUS_DATA_DIR>/docs/templates/bug-triage-table.md` |
| Every `workflows/*.md` (all nine files, including templates and handover docs) | `<BUS_DATA_DIR>/docs/workflows/<original filename>` |

Links and `DOCS` paths in the guides address the generated docs folder. The
repository layout is given by the first column above. Bus refreshes the docs
from its embedded content when preparing an orchestrator launch; files and
directories are owner-only. The filled system prompt belongs to that launch's
callback folder, and the provider receives it again on resume.

`workflows/create.md` is the full workflow creation and revision guide.
`skills/workflow-create/SKILL.md` is only its skill entrypoint; the binary never
embeds that pointer. This README documents the contract and is not embedded.

## Build input closure

Cargo source archives must include `orchestration/` and `workflows/`; the Nix
build fileset includes the same directories. These are build inputs. Missing source files
fail compilation through `include_str!`, and an unknown or unfinished
`{{...}}` placeholder in the compiled default prompt fails its constant
assertion. The documentation's literal placeholder examples are left intact.

The orchestration tests enumerate the repository's workflow directory so a new
file missing from the bundle fails, check the docs and rendered placeholders,
and compare emitted bytes at unrelated output locations and after refresh. The
default content comes from the binary regardless of the process's working
directory or where the executable is installed. Explicit custom prompts keep
their existing per-launch filling behavior.

## Prompt placeholders

Bus replaces these placeholders in an orchestrator prompt:

| Placeholder | Value |
|---|---|
| `{{ROOM_NAME}}` | The work room's name |
| `{{ROOM_ID}}` | The work room's numeric id |
| `{{AGENT_NAME}}` | The orchestrator agent's name |
| `{{DOCS}}` | The absolute path to that Bus session's generated docs folder |
| `{{WORKFLOWS_DIR}}` | The detected checkout's absolute workflows path for a dev build or `--dev` run, otherwise `<BUS_DATA_DIR>/docs/workflows` |

The add-agent preview can show an unassigned room; an actual MASTER agent
requires a work room through `--orchestrates`. Explicit per-launch custom
prompts from the TUI, `--system-prompt` or `--system-prompt-file` still use the
same placeholder filling and provider delivery. They do not replace the
compiled-in documentation library.

## Control CLI

Every session runs the control socket and answers the agent-tier commands;
dev tools answer only in a session started with `--dev` (`docs/dev-tools.md`).
Orchestrators drive Bus
through the control CLI documented in `how-to-bus-cli.md`; they do not edit
room state files or use the server API directly. Send work as the orchestrator
with `bus send --room ROOM --as AGENT --to WORKER --async --text TEXT`, and
maintain the workflow path and progress in the room notes. The binding rules
and working guide are in `rules.md` and `guide.md`.
