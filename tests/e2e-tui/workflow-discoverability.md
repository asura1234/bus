# Workflow discoverability QA

## Run contract

Run each case in a fresh scratch Bus with fake providers, isolated HOME and
BUS_DATA_DIR, a dedicated target/workflow-docs build, and a 160x45 terminal with
graphics disabled. Never use the running Bus, target/debug, or desktop automation.
Use the e2e-test skill and its scratch TUI driver. UI input creates the
orchestrator; read-only state, launch files and filesystem reads corroborate the
prompt and its referenced files. Retain copies of the prompt and rules as evidence
before stopping the scratch session. No SKIP allowances.

For WF-DEV use the dev-tools debug binary built from this checkout. For WF-RELEASE
use a release-profile dev-tools binary copied outside every Bus checkout, run from
a directory with no Bus sources. The dev-tools feature supplies only the scratch
driver; workflow embedding and path resolution are shared with installed builds.
The relocated binary must have no source checkout in its executable or working
directory ancestors. Use the driver's fake providers; no live model is needed.

## Cases

### WF-DEV — Source launch discovers repository workflows

**Setup:** start a fresh scratch session with the development binary. Record the
checkout's absolute path and the driver's scratch workspace path.

1. Click MASTER and press Ctrl+N. Fill the name `wf-dev`, choose Claude Code,
   set PWD to the scratch workspace (complete its path suggestion before leaving
   the field), and select the default work room in Orchestrates room. Leave the
   system prompt unedited and click Add. Accept hook consent only for that scratch
   workspace if shown. → An orchestrator exists in MASTER, targets the default
   work room, and its fake Claude terminal opens.
2. Read the orchestrator's Bus-owned launch system-prompt.md. → Near the top it
   names Never act on assumptions; ask the developer and links to the first
   binding rule; its Workflows reference is the absolute checkout/workflows/
   path, and it links to the named-workflows rule. No unresolved placeholders
   remain. The referenced auto-merge-pr.md exists and matches the source bytes.
3. Read the referenced orchestrator-rules.md and the session docs/workflows/
   directory. → The first rule names Acting on assumptions, contains concrete
   triggers and self-checks, requires a MASTER [waiting-on-you] question and holds
   dependent work until answered, and tells workers to ask the orchestrator. The
   named-workflows rule uses the same directory as the prompt. Every repository
   workflow exists under its original name and byte content in the session copy.

### WF-RELEASE — Relocated release launch discovers bundled workflows

**Setup:** start a fresh scratch session using the relocated release-profile
binary and no source checkout in either launch anchor. Record the binary path,
working directory, and scratch data path.

1. Click MASTER and press Ctrl+N. Fill the name `wf-release`, choose Claude Code,
   set PWD to the scratch workspace (complete its path suggestion before leaving
   the field), and select the default work room in Orchestrates room. Leave the
   system prompt unedited and click Add. Accept hook consent only for that scratch
   workspace if shown. → An orchestrator exists in MASTER, targets the default
   work room, and its fake Claude terminal opens.
2. Read the orchestrator's Bus-owned launch system-prompt.md. → Near the top it
   names Never act on assumptions; ask the developer and links to the first
   binding rule; its Workflows reference is the absolute scratch
   BUS_DATA_DIR/docs/workflows/ path, and it links to the named-workflows rule.
   No source-checkout path or unresolved placeholder appears. auto-merge-pr.md
   exists there and matches the embedded source bytes.
3. Read the referenced orchestrator-rules.md and session docs/workflows/. → The
   first rule and named-workflows rule have the same contract as WF-DEV; the rule
   uses exactly the directory named by the prompt. Every repository workflow is
   present under its original name and byte content, including agent-handover.md,
   cross-repo-feature.md and handover-template.md.
