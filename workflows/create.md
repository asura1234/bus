# workflow-create

A workflow is a living plan for one Bus room. You draft it with the developer before
kickoff and rewrite it whenever it stops matching reality. Nothing executes it;
it is the shared picture of the work.

`DOCS` below is the Bus docs folder named in your system prompt, where Bus
writes this guide as `workflow-create.md`.

## 1. Gather what you need

Find out, asking the developer only for what you cannot discover:

- **Goal and done check:** what done looks like and how the developer will verify it.
- **Non-goals.**
- **Scope:** repositories and branches; hard gates (benchmarks, CI, deadlines).
- **People and models:** the room's agents, their providers, usage and
  compactions (`bus state`). What the developer must do themselves (manual tests,
  approvals).
- **Authority:** what you may decide alone and what goes to the developer.

## 2. Pick a shape

Start from the closest pattern and adapt it. Worked examples live in
`DOCS/workflows/`.

| Pattern | Use when | Shape |
|---|---|---|
| Sequential | Each step needs the previous result | A → B → C |
| Fan-out / fan-in | Independent pieces, then a merge | split → parallel agents → merge |
| Pipeline | Many items through the same stages | each item moves on as soon as it is ready |
| Explore then commit | Uncertain approach | research → parallel spikes → measure against gates → pick |
| Best-of-N | A judgment call | N agents propose independently → compare → pick or ask the developer |
| Review loop | Code must converge | reviewers on different models → author fixes → repeat until all ready, capped |
| Cross-repo | Same feature on several platforms | one agent per repo in parallel, shared spec, joint review |

Always add:

- **Verification:** every build step is checked by someone other than its
  author (tests, a reviewer, the developer).
- **A failure route:** where work goes when a gate fails (back to research, a
  rethink with the developer), not just the happy path.
- **A cap:** maximum review rounds or attempts before you stop and ask the developer.

## 3. Write workflow.md

Copy `DOCS/templates/workflow-template.md` to `DOCS/../workflows/<room>.md`,
next to the docs and outside every repository, unless the developer wants it
elsewhere (for example tracked in a repository). Fill every section:

- **Graph:** mermaid `flowchart TD`. Nodes are agent tasks named
  `agent: task`, diamonds are decisions with labeled edges, red (`:::developer`)
  nodes wait for the developer. Group parallel work in subgraphs. Mark the node the
  room is on with `:::current`.
- **Participants:** agent, provider, role, worktree or branch. Reviewers on
  different providers.
- **Coordination:** shared branch, separate worktrees (created by an agent with
  `worktree-new`), or "ask me before writing file X" for a few shared files.
  For each worktree, record the checkpoint through which it must be retained
  and where needed baselines and metrics will be archived. Delegate free-space
  checks on the data volume at kickoff and each phase boundary; below 30 GiB,
  hold builds, benchmarks and large copies until cleanup restores space.
- **Gates** and **decision rules** from step 1.
- **Log:** one dated line: "Drafted with the developer."

Put the file's absolute path on the first line of the room notes.

## 4. Confirm, then start

Show the developer the graph and anything you assumed. Start when they agree.

## 5. Revise

Rewrite the file, not just your plan, when:

- a step fails or a gate cannot be met,
- the developer changes the requirements,
- you add, remove or replace an agent, or change coordination.

For each revision: update the graph and the affected sections, move
`:::current`, add a dated log line with the reason (newest first), tell the
affected agents, and update the room notes. If the change alters the goal or
needs authority you do not have, ask the developer first.
