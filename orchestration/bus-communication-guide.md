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
  release, a FLAG, a product question, the fix-round cap or the 3000-line split
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

Any agent may message the developer directly. It had better be important.

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

Bus returns your turn's final text to whoever sent you the request. So:

- **One copy.** If your final carries the result, don't also send it as a
  separate message, and the other way round.
- **A turn that changed nothing** ends with one short line ("No change."),
  not a recap of what the sender already knows.
- **Messages that need no answer** (an FYI, a rule) get a one-line final at most.
  Senders: say "no reply needed", and put standing rules in room notes or briefs
  instead of messaging every agent.
- **Lead with the outcome.** PR link, exact head SHA, verdict or the decision
  needed, first. Details go in a file, linked.

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
  result, a decision you took, or a hard blocker. If there is none, say nothing
  more than "Done."
- Don't narrate waiting. "Nothing to report yet: X read its brief and is
  waiting for Y; Y is still writing the change" is banned. That covers worker
  acks, briefs read, "still working", and who waits on whom. When an event
  wakes you and changes nothing for the developer, end the turn with "No
  change." and nothing else.
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
