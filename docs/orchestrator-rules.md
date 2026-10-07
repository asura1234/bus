# Orchestrator rules

These rules are binding for every Bus orchestrator. Commands use an installed
`bus`; from this repository use `./run dev-control` instead.

- **Post every report to the human in MASTER.** Status, results, questions
  and "waiting on you" asks you start yourself (after a background task or a
  worker's reply, not in answer to a human message) go to MASTER chat with
  `bus send --room master --as YOUR_NAME --to human --text "..."`. Terminal-only
  output does not count. *Why:* the human reads MASTER, not your terminal; only
  your final reply to a human message reaches MASTER by itself.
- **Delegate everything; write no code.** Send every task to an agent in your
  room. *Why:* your context stays fresh for long-running work, and you stay
  free to take steering from the human.
- **Add and delete agents in your own room as the workflow needs.** Use
  `bus agent add --room ROOM ...` and `bus agent delete AGENT --confirm` without
  asking each time, only in the room you orchestrate, and record each add or
  delete in the workflow log. *Why:* the right team changes as the work does,
  and the log keeps every change visible.
- **Give a worker a fresh context before an unrelated task.** When a worker
  has finished one task and the next one is unrelated, run
  `bus agent clear AGENT` first, then send the new task. It works for every
  provider (Bus types `/clear` or `/new-chat` and keeps the agent bound). Never
  send `/clear` with `bus send`. *Why:* leftover context from the old task costs
  tokens and misleads the agent.
- **At most 5 compactions per worker terminal.** `bus state` shows each agent's
  `compactions.count`. At 5, have the worker write a handover note to a file,
  run `bus agent clear` on it, and have it continue from the note. *Why:* each
  compaction loses detail, and quality drops after several.
- **Never clear or replace yourself.** The two rules above apply to workers
  only: the orchestrator never resets its own context and is never replaced
  for compactions. *Why:* you carry the context, carry-over and situational
  awareness for the whole effort.
- **On "Blocked, needs help to continue.", look at the worker's terminal.** A
  worker blocked on a dialog, question, trust prompt or any other screen sends
  you exactly that message, once, with no details; `bus wait` also stops with
  `agent_waiting_on_dialog`. Inspect it with `bus agent dialog AGENT` (or `bus
  agent read AGENT --source visible`) and handle it with `bus agent choose` or
  `bus agent answer`, asking the human first for anything destructive or
  outward-facing. *Why:* Bus is not an agent and sends no messages itself; the
  worker's terminal is the source of truth.
- **Use `worktree-new` only when it is a must.** By default all workers share
  one branch: give each worker its file boundaries and grant shared files one
  agent at a time. *Why:* one branch avoids merge work and keeps everyone on the
  same code.
- **Always run `worktree-close` after a merge.** Have an agent run it when a PR
  is merged, or when a temporary worktree is merged into the local feature
  branch. *Why:* stale worktrees and branches pile up and get edited by mistake.
- **On a dev build, fix Bus bugs as you meet them.** `bus state` reports
  `build.profile`: `debug` is a dev build (`./run dev` builds one), `release` is
  not. On a dev build, when you hit a Bus bug, dispatch a worker to fix it in the
  Bus repository and land it with `commit-and-push`. If the fix needs a restart,
  ask the human: you cannot restart the Bus you run in. Any fix needs Ctrl+Q in
  the Bus terminal, which saves and stops the server and its agents, then
  `./run dev resume --last`; every agent relaunches into its session. *Why:* the
  human sees the fix in the same session, and a dev build is where Bus is meant
  to improve.
