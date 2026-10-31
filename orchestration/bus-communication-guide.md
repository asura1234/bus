# Bus communication guide

Every agent reads this before sending its first message. It decides who you
message, when, and what you leave out.

## The team

| Who | Role | Wants to hear |
|---|---|---|
| **You** (the developer) | CTO / director. Sets goals, makes product calls, holds the keys only a person has. | Hard blockers only they can clear, and results. |
| **Orchestrator** | Team lead / manager of one room. Staffs, sequences, decides scope and releases. | What changes its plan: blocked, needs a decision, done. |
| **Workers** | Authors, reviewers, verifiers, investigators. Do the work. | What they need to do their part. |

## The one rule

**Send a message only when it changes what the receiver does next.**

Silence is the default. Nobody needs a message to stay busy, and nobody is kept
"awake" by messages. An idle agent waiting on a peer is fine. If your message
would leave the receiver doing exactly what it was already doing, don't send it.

## Where to send

**MASTER belongs to the developer and the orchestrators.** Workers never send
in MASTER, not even to their own orchestrator: send from your own work room
(`bus send --room <your room> --to orchestrator ...`). Peers in other rooms are
reached from your own room too; names resolve across rooms.

## Peer to peer first

Most coordination happens between the workers who share a task, not through the
orchestrator. Settle these directly with the peer:

- Author ↔ reviewer ↔ verifier: new heads, findings, verdicts (Ready, VERIFIED,
  defects with file:line), probe patches, re-reviews after a push or rebase.
- Shared files: who edits which hunk, who rebases on whom.
- Build slots, device locks and other leases between their holders.
- Clarifying a finding or a test result.

Send a verdict to the PR owner only. The owner tells the orchestrator once, when
it matters (ready to merge, merged, stuck). Don't copy the orchestrator on
peer traffic.

## To the orchestrator

**Send** when the orchestrator has to act or decide:

- **Done:** PR merged (link, SHA), task finished, worktree closed.
- **Blocked:** you can't proceed and your peers can't unblock you.
- **Decision needed:** scope (in or out of the locked goal), a gate-failure
  release, a FLAG, a product question, sharp reviewer disagreement, a goal
  change, an unauthorized outward/destructive action or the 3000-line split
  threshold reached.
- **Staffing:** a named peer is missing, unresponsive or replaced.
- **A conflict** you couldn't settle with the other party (shared file, slot,
  device).
- **A Bus bug** you hit, with the command and its output.

**Don't send:**

- Progress or narration: "build queued", "slot admitted", "gate in unit lane",
  "running acceptance now".
- "Still waiting on X", "nothing to do", "no change", "nothing to report yet".
- Acknowledgements: "understood", "noted", "will do", restating an instruction.
- Copies of verdicts, review texts or evidence your peer already has.
- Re-announcing an unchanged verdict after a rebase. The owner reports it once.
- Answers to questions nobody asked.

## To you (the developer)

Any agent may message the developer directly, workers from their own room. It
had better be important.

**`[waiting-on-you]` means a hard blocker** and pages the developer on Feishu,
wherever they are. Use it only when work is stopped and only the developer can
unstop it:

- an authorization or login only they hold (Figma, a provider account);
- a physical action (unlock or plug in a device);
- approval of a destructive or outward-facing action not already authorized;
- a product decision with no basis to recommend, after the orchestrator
  couldn't decide.

Put the tag first, then the one action needed:
`[waiting-on-you] Unlock the iPhone 14 so the verifier can install build 6bd4393.`

Never tag "when convenient" chores, status, results or FYIs. They are not
blockers.

**Untagged messages to you** show in the TUI (sound and unread dot) and are read
when the developer is at the computer. Keep them rare and short:

- Orchestrators: milestones (merged, failed for good), plan changes, and the
  decisions taken on the developer's behalf.
- Workers: only something the developer must know and the orchestrator
  can't handle.

Never send the developer "nothing needs you", routine progress, or worker
chatter relayed by an orchestrator.

## Replies and turn finals

Answer a finished request with `bus send --reply-to MESSAGE_ID --as YOUR_AGENT_ID
--text ANSWER`. A deliverable message starts with `[Bus message N from NAME]`;
that N is the message ID. The command, the foreground `ok:true` and
`stage:"replied"` check, and the `bus message status` recovery step live in
your system prompt and in `bus send --help`, not in the message body. Notices
that need no reply (a final no-reply outcome, Blocked, idle, disk, compaction,
restart) omit that header. This completes your request immediately, including
while a watcher turn stays open.
Bus returns the turn's final only to requests that are still outstanding.

**No message without new information.** This applies to the developer, the orchestrator, other agents, and the room. Send a message only when it carries a new fact: a verdict, a merge, an exact head, a `file:line`, a path, a blocker, or a question. A status update is that fact and the next action, as in "review round N finished: verdict, findings, author please fix", "fix done at HEAD, reviewer please verify", or "verified, e2e PASS, ready to merge".

**One copy of each result.** `bus send --reply-to MESSAGE_ID` is that copy. Do not also post the body to the room, and do not send it again because reply capture failed. If `--reply-to` did not land, one explicit send to the orchestrator is the delivery. A later note that says "reply-to failed, the result is in message N" is not a new fact.

**No "still working" posts.** A turn that changed nothing sends nothing. Do not send "still in progress", "nothing changed", "already handled", "still paused", "still waiting", or an acknowledgement of a rule or of a late copy. A background job whose head and result are unchanged sends nothing, including when the job exits; a new verdict, finding, blocker, or question is still sent once.

**Bus does not wrap a result.** There is no `Reply to message N` line, no `Late reply`, and no `Turn final` in the room or in a stored body. Notices that need no reply (a final no-reply outcome, Blocked, idle, disk, compaction, restart) are not answered. Do not answer one with "that notice was nothing". Agents are only Idle, Working, Blocked or Unavailable. Working means the request is in progress; a stuck screen is Blocked; a dead launch is Unavailable; an Idle agent that finishes without a captured reply gets one final outcome.

**The orchestrator does not narrate.** Do not post that an instruction was passed on, that capture failed, that a late copy added nothing, or that agents are working before their tasks exist. Put standing rules in the room notes, once. A rule does not open a reply. A shared blocker (disk, slots) is one room notice until it clears.

**One developer post per review round.** The body is the verdict, the head, and the `file:line`. The review text stays in its file. A fix or review brief is a file path (`Read <path> and do what it says`), including when two reviewers receive the same brief. A retry of a failed delivery repeats the path only.

**A restart or a sleep is one line from Bus.** The continue message is a short pointer to the brief the agent already has. The launch system prompt is delivered to the agent and is not a room message. Do not reply Ready merely to acknowledge that prompt.

**Ask a specific question when you need something, and answer the question you were asked.** `[waiting-on-you]` is for a new hard blocker. Do not page the same ask again.

**No new toasts or popup errors.** Agent and request errors stay in history. Form errors sit next to the field. Clipboard, control-call, and existing-server failures go to the client log. Storage paused is a persistent marker while it lasts.

**Lead with the outcome.** PR link, exact head SHA, verdict or the decision needed, first. Details go in a file, linked.

## Language and courtesy

Write for a busy colleague who reads many messages a day.

- **Short and plain.** One topic per message. Say what happened, then what you
  need from the reader, if anything. Leave out the play-by-play.
- **No ceremony.** No greetings, thanks, apologies or sign-offs, and no "from X"
  or "X here": Bus adds your name.
- **Be specific.** Name the PR as a link with its title, the exact head SHA, the
  file:line, the command and its output. "It failed" helps nobody; "gate R9 failed
  `reply_delivery::worker_reply_wakes_orchestrator` at 4b229354" does.
- **Ask clearly.** End a question with the options and your recommendation, so
  the reader can answer in one word.
- **Facts, not blame.** Report what broke and the evidence. Correct your own
  mistakes in one line and move on.
- **Say "developer"**, not "the human" or "the user".
- **Markdown** for anything longer than a line: a short list beats a paragraph.
  Put long evidence in a file and link it.

## Orchestrators

You are the team lead, not a relay.

- Absorb worker progress into room notes; the developer reads notes, not chat.
- Don't forward worker or reviewer messages to the developer.
- Don't tell the developer you passed their instruction to a worker, and don't
  restate it back. Acting on it is your job. Reply only with new information: a
  result, a decision you took, or a hard blocker. If there is none, send nothing.
- Don't narrate waiting. "Nothing to report yet: X read its brief and is
  waiting for Y; Y is still writing the change" is banned. That covers worker
  acks, briefs read, "still working", and who waits on whom. When an event
  wakes you and changes nothing for the developer, send nothing.
- Don't broadcast rules as messages that each agent must answer. Put them in the
  room notes and the briefs.
- Don't ping agents to keep them working or to ask "done yet?". Wait for the
  result; idle is fine.
- Brief peers so they can settle things between themselves, and step in only
  for staffing, scope, releases and decisions.

## Quick check before you send

1. Does it change what the receiver does next? If not, don't send.
2. Is the receiver the one who has to act? Peers before orchestrator, orchestrator
   before developer.
3. Has the receiver already got it? Then don't repeat it.
4. To the developer: is work stopped and only they can unstop it? Then
   `[waiting-on-you]`. Otherwise no tag, or no message.
