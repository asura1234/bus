# Best-of-N adjudication principles

N agents gave N conflicting proposals for the same technical question. This skill converges them
into **one decision** and leaves a **ranked record**, so that when the first-place approach fails you
can switch directly to the second place instead of reinventing.

## What this skill solves, and what it does not

**It only solves disputes over technical approaches**: the problem is confirmed to exist, the goal is
locked, and the dispute is about **which technical approach to adopt**.

**Triage does not belong to this skill.** Deciding "who should decide this" is the orchestrator's
job — it may be an orchestrating agent in Bus, or the developer. It judges:

- a technical question → assign one agent to run this skill;
- fix in this PR or open another, what goes first, priority, owner → **the orchestrator decides itself**.
  It holds exactly this kind of workflow-level context, and N agents' opinions on "when to do it" are not evidence;
- product trade-offs, user preferences → ask a human;
- "is the problem real at all" → go back to verification: the first-party evidence of
  `address-review-comments`, the "test that goes red" of `review-pr`. Until a dispute over approaches
  is established, do not come here.

So the disputes you receive **have by default already been judged technical**. If after starting you
find one is not — what is really stuck is scheduling or a product trade-off — **stop and hand it back
to the orchestrator, do not decide where it should go yourself**, and do not force a ranking on it.
Adjudicate and persist the other disputes as usual; one dispute going back does not leave the whole round hanging.

The test is simple: **if all N proposals were implemented, would the resulting code differ?** If yes,
it is a technical dispute; if not (only the timing, order, or ownership differs), hand it back.

## What "best" means

Every ranking is subordinate to one sentence: **most likely to achieve the locked goal**. The goal
itself does not vote — it is locked by the plan's `## 目标`, the PR's `.locked-goal`, or one sentence
the developer gives on the spot, and the N proposals are just different paths to it.

### Eligibility first (binary, not part of the ranking)

An option is **disqualified**, rather than ranked last, when it:

- fails to achieve the locked goal;
- hits a locked non-goal;
- violates any hard rule in the [architecture principles](../../docs/guides/architecture-principles.md)
  (wrong layer, reversed dependency, fallback for an impossible state, a pointless deprecation transition period...);
- requires overturning an archived decision.

A disqualification must state which of the above it is. "I don't like it" is not a disqualification.

### Then rank the survivors

1. **Simplest** — the fewest new concepts, files, abstractions, branches, state. Compare against
   **what the repository already has**, not against some ideal form. An approach that reuses an
   existing owner is simpler than one that creates a new owner, even with more lines of code.
2. **Most direct** — the fewest layers of indirection between root cause and fix. Fixing the sole
   establisher directly > adding a bypass on the consumer side; one path > one path plus a fallback;
   deleting a redundant branch > adding another branch to handle it.
3. **Most likely to succeed** — the fewest unknowns, precedent in this repository, the smallest blast radius if wrong.

### Decisive rule: complexity carries the burden of proof

**The default winner is the simplest one.** To make it lose, you must **name a concrete failure
mechanism** — which input, which concurrent timing, which platform, which line makes it not hold.

"Might not be robust enough", "might not be enough in the future", "more extensible" without a
mechanism **are not reasons**. That is exactly the generic rhetoric by which complex options win every
argument: it is always true and never refutable, and therefore should never be accepted as evidence.

The reverse holds too: if the simplest option really has a named failure mechanism, it should lose —
"simple" is not a reason that overrides a known breakage. Criterion 3 is a real criterion, not a consolation prize.

## Agreement is not exemption: the candidate set is not exhaustive

N models independently giving the **same** proposal is a strong convergence signal — the same ground
truth, the same locked goal, different intelligences converging on the same point usually means it
really is right. It is also the most common case and should not go through the full ranking ritual:
one option, all sources, eligibility passed — record it and move on.

But agreement has one failure mode unique to it, **hidden precisely by the agreement itself**:

> The option nobody proposed cannot win.

N models share a lot of training priors; they will think of the same respectable, structured,
professional-looking approach together, and together fail to think of the five-line fix. In that case
the ranking's "unanimously passed" is true, but "this is the simplest approach" is false — because the
simplest approach never entered the candidate set.

So when **all candidates are non-simple** (all adding abstractions, new paths, new state), the
adjudicator must answer one question before persisting: *is there a simpler way that nobody proposed?*
Common forms:

- fix the sole establisher directly, instead of adding handling at every consumer;
- delete the branch that creates the problem, instead of adding another one to handle it;
- let an existing owner take on a bit more, instead of creating a new owner;
- do nothing — the problem is a symptom of another defect; fix that one.

If you think of one, add it to the candidate set as a new option; it goes through eligibility and the
burden of proof as usual. If you cannot, write one line in the record: "Checked, no simpler form."
That line is not ritual: it distinguishes "I checked" from "I never thought about it", and weeks later
the two look exactly the same.

## The adjudicator is necessarily a participant

The orchestrator — an orchestrating agent in Bus, or the developer — deliberately does not hold
technical details, so **it does not adjudicate**. What it does is routing and bookkeeping: assign who
adjudicates, hand the N proposals over, take the record back, and remember what has been tried.

So the adjudicator is always one of the agents that took part in the fan-out, judging approaches it
proposed itself. This is not an exception that needs special approval but **the default state** — so do
not treat it as an occasional risk; write it into the record as a standing constraint:

- `adjudicator` states who it is;
- if the chosen option lists the adjudicator as a source and there is more than one surviving option,
  `self_selection_note` is required: **name the concrete mechanism on which the simpler rival loses**.

The standard is not relaxed by a single word; it is only written down. Note that what is required is
"why the rival fails", not "why my approach is good" — the latter can always be written, the former can
be refuted, and only refutable reasons are evidence.

The adjudicator should not write the record for the orchestrator either: it cannot read the technical
details and should not be asked to re-check the ranking. The record is written for **the next person or
agent who takes over** — especially the time, weeks later, when the first place fails and someone needs to switch to the second.

## No blending

**By default one option must be adopted whole.** A synthesis of N proposals is almost always the union
of their complexities — that is exactly the mechanism by which "design by committee" produces
over-engineering, and here there happen to be N agents, each very persuasive.

Only one case permits a third approach absent from the original proposals: it is **simpler than every
option it borrows from**. It then enters the ranking as a new option and, like the others, goes through
the eligibility check and the burden of proof above, with no "it synthesizes everyone's views" exemption.

## Three conclusions: there is always a ranking; decisiveness is the variable

**Every round produces a total-order ranking**, without exception. The verdict does not say "whether a
ranking was produced", but **how far the first place leads**:

### universal —— every source independently gave the same proposal, and it holds up under verification

Not "I am sure", but two mechanically checkable conditions: the chosen option was independently
recommended by **every** source, **and it is the only surviving option** — if anyone proposed something
else (even ranked second and not disqualified), it is not "everyone gave the same proposal"; judge clear
or toss-up by whether it pulls ahead.
This is the strongest convergence signal (same ground truth + same locked goal → different intelligences
landing on the same point), and also the most common case; it does not need the full ranking ritual.
The only extra step is the "Agreement is not exemption" section: when all candidates are non-simple,
first confirm there is no simpler way that just nobody proposed.

### clear —— the first place pulls ahead of the rest

`beats_runner_up` must name a **concrete failure mechanism**. If you cannot write a mechanism, it is
not clear — that means it did not actually pull ahead of the second place; judge toss-up honestly. This
rule allows no leeway: the difference between clear and toss-up is "whether there is a refutable reason",
not how confident the adjudicator is.

### toss-up —— the first place does not lead by much

The ranking is still a total order; the gap is just small. **This is not "cannot decide", and even less
a stop.** The next step is still to do the first place first — the value of the ranking is producing an
action so the work does not stall, **even if this order is essentially arbitrary**: when you really
cannot separate them, the difference by definition does not matter, and setting an order at random and
moving on is far cheaper than stopping here to wait for a better reason.

The difference is only in two points: the second place is genuinely alive (once the first place goes
badly, switch early, do not force it), and `toss_up_reason` must be written — **what evidence is still missing to separate them**.

`toss_up_reason` is written for the orchestrator. **This skill does not decide what to do next**:
opening an isolated worktree per candidate for a spike, choosing one by another constraint such as
schedule/cost, or asking a human, are all the orchestrator's job. Your output is only "where the gap is
and what evidence is missing".

(If a spike really is needed: fix the criterion first — what result counts as success. Otherwise what
comes back is N copies of "mine works", which is the same argument moved somewhere else. And a spike only
answers "does it work", not "which is simpler"; when both hold, the ranking criteria apply as usual, and
an approach gets no bonus for "already being written" — that is sunk cost.)

## Why keep the ranking, not just the winner

When the first-place approach fails, an agent's default move is to **think up a new one** — that is
entropy: the new approach did not go through this round's eligibility check, nor through anyone's
challenge, yet looks more promising than the one that just failed because "it is new".

Keeping the full ranking turns this step into a lookup: A fails → read B's TLDR → do B directly. B has
already passed eligibility, has already been seen by N agents, and the concrete reason it lost to A is
already known — and that reason may have just been overturned.

Therefore:

- **Every submitted option must appear in the ranking**, including obvious losers. Dropping an option
  drops a future fallback, and at the moment of dropping nobody thinks it will be needed.
- **Every option needs a TLDR** — one or two sentences on what exactly it proposes, so that weeks later
  nobody has to go back and reread the original proposals.
- **The failure trigger must be stated** — what "A fails" means must be defined on the spot. Without a
  definition, the moment to fall back becomes a subjective judgment, and at that moment people tend to
  give A one more chance, one more patch.
- **The ranking allows no ties, but the conclusion can be toss-up** — the two do not conflict. When you
  cannot separate them, honestly judge toss-up while still producing a total order (even if nearly
  arbitrary): the former is honesty, the latter is so there is a next step. What must really be avoided is
  giving neither a conclusion nor an order — that is postponing the decision while disguising it as made.

## When switching to the next place

Switching is not reconvening the meeting. Read the next place's TLDR and its "reason it lost to the
previous place" in the record, and confirm whether that reason has been overturned by the failure just now:

- Overturned → execute the next place directly, and append a line to the record saying what A's failure triggered.
- Not overturned → the root cause of the failure is not "the wrong approach was chosen" but something
  else. In that case **do not** slide down the ranking; go back to the problem itself and gather evidence
  again. The ranking protects against "reinventing", not "running through the list".
