"""Best-of-N ledger structural validation: collects every error in one pass, shared by best_of_n.py's validate / render / attempt."""

from __future__ import annotations


VERDICTS = frozenset({"chosen", "ranked", "disqualified"})
UNIVERSAL, CLEAR, TOSS_UP = "universal", "clear", "toss-up"
DISPUTE_VERDICTS = frozenset({UNIVERSAL, CLEAR, TOSS_UP})


class LedgerError(ValueError):
    """The ledger is structurally invalid."""


def _dispute_errors(dispute: dict, index: int) -> list[str]:
    where = f"dispute[{index}]"
    errors: list[str] = []
    did = dispute.get("id")
    if not did:
        errors.append(f"{where}: missing id")
        return errors
    where = f"dispute {did}"

    if not dispute.get("question"):
        errors.append(f"{where}: missing question")

    verdict = dispute.get("verdict")
    if verdict not in DISPUTE_VERDICTS:
        errors.append(
            f"{where}: verdict must be one of {sorted(DISPUTE_VERDICTS)} — a ranking always exists; the verdict says how decisive it is"
        )

    options = dispute.get("options") or []
    if not options:
        errors.append(f"{where}: a dispute must have at least one option")
        return errors

    seen: set[str] = set()
    malformed = [option for option in options if not isinstance(option, dict)]
    for option in malformed:
        # A bare string makes every later `.get` raise AttributeError, which `except LedgerError`
        # does not catch — a corrupt record then becomes a traceback instead of a fixable validation error.
        errors.append(
            f"{where}: options contains an entry that is not an object: {option!r}. "
            'Each option is an object of {"id", "tldr", "sources", "cost", "verdict", "rank"}'
        )
    if malformed:
        # Exit early on **this one** shape only: writing the condition as `if errors` would let any
        # earlier unrelated dispute-level error (missing question, invalid verdict) skip the whole
        # option-level validation, so tied ranks and missing beats_runner_up go unreported. record's
        # contract is "FAIL + every reason, fix once and it passes"; reporting one at a time turns it
        # into N round trips.
        return errors

    ranked: list[int] = []
    chosen: list[str] = []
    for position, option in enumerate(options):
        oid = option.get("id")
        if not oid:
            errors.append(f"{where}: option[{position}] missing id")
            continue
        if oid in seen:
            errors.append(f"{where}: duplicate option id: {oid}")
        seen.add(oid)
        if not option.get("tldr"):
            errors.append(f"{where} option {oid}: missing tldr — switching relies on it; you cannot go back and reread the original proposals")
        option_verdict = option.get("verdict")
        if option_verdict not in VERDICTS:
            errors.append(f"{where} option {oid}: verdict must be one of {sorted(VERDICTS)}")
            continue
        if option_verdict == "disqualified":
            if not option.get("disqualified_reason"):
                errors.append(f"{where} option {oid}: a disqualification must state which rule it hit")
            if option.get("rank") is not None:
                errors.append(f"{where} option {oid}: a disqualified option must not have a rank")
            continue
        rank = option.get("rank")
        if not isinstance(rank, int) or isinstance(rank, bool) or rank < 1:
            errors.append(f"{where} option {oid}: rank must be an integer >= 1")
            continue
        ranked.append(rank)
        if option_verdict == "chosen":
            chosen.append(oid)
            if rank != 1:
                errors.append(f"{where} option {oid}: the chosen option's rank must be 1")

    if len(chosen) != 1:
        errors.append(
            f"{where}: there must be exactly one chosen option (currently {len(chosen)}) — a tie or a gap postpones the decision while disguising it as made"
        )
    if ranked and sorted(ranked) != list(range(1, len(ranked) + 1)):
        errors.append(
            f"{where}: ranks must be a permutation of 1..{len(ranked)}, got {sorted(ranked)} — no ties and no gaps allowed"
        )
    if verdict == CLEAR and len(ranked) >= 2 and not dispute.get("beats_runner_up"):
        errors.append(
            f"{where}: judging clear requires stating why the first place beats the second, naming a concrete failure mechanism"
            " — if you cannot write a mechanism it did not actually pull ahead of the second place; that is toss-up"
        )
    if verdict == TOSS_UP:
        if len(ranked) < 2:
            errors.append(f"{where}: toss-up needs at least two neck-and-neck options (currently {len(ranked)})")
        if not dispute.get("toss_up_reason"):
            errors.append(
                f"{where}: toss-up requires toss_up_reason — **what evidence is still missing to separate them**. "
                "The orchestrator relies on it to decide whether to spike, choose by another constraint, or ask the developer; "
                "saying only \"they are about the same\" throws the decision back without explaining why"
            )
    if not dispute.get("failure_trigger"):
        errors.append(
            f"{where}: missing failure_trigger — without it the moment to fall back becomes a subjective judgment, and at that moment people tend to give the first place one more chance"
        )
    return errors


def _consensus_errors(dispute: dict, adjudicator: str, all_sources: set[str]) -> list[str]:
    """The two "looks passed" shapes: agreement and self-selection.

    Agreement's unique failure mode is a non-exhaustive candidate set: N models share priors, think of
    the same respectable approach together, and together miss the five-line fix. Then "unanimously
    passed" is true and "this is the simplest" is false.
    Self-selection is the default state rather than an exception — the orchestrator has too little
    context, so the adjudicator is necessarily one of the participants.
    """

    where = f"dispute {dispute.get('id')}"
    errors: list[str] = []
    survivors = [o for o in dispute.get("options", []) if o.get("verdict") != "disqualified"]
    chosen = next((o for o in survivors if o.get("verdict") == "chosen"), None)
    if chosen is None:
        return errors

    for option in dispute.get("options", []):
        unknown = set(option.get("sources") or []) - all_sources
        if unknown:
            errors.append(f"{where} option {option.get('id')}: sources {sorted(unknown)} are not in this round's sources")

    if dispute.get("verdict") == UNIVERSAL:
        if set(chosen.get("sources") or []) < all_sources:
            errors.append(
                f"{where}: judged universal, but the chosen option was not independently recommended by every source"
                " — universal means \"everyone gave the same proposal, and it holds up under verification\", not \"I am sure\""
            )
        if len(survivors) > 1:
            # Another surviving option means not everyone gave the same proposal: someone proposed
            # something else. Judge clear (the first place pulled ahead of it) or toss-up (it did not), not universal.
            errors.append(
                f"{where}: judged universal, but there are still {len(survivors) - 1} other surviving option(s)"
                " — if someone proposed something else, it is not \"everyone gave the same proposal\"; judge clear or toss-up by whether it pulls ahead"
            )

    for option in dispute.get("options", []):
        if not [src for src in (option.get("sources") or []) if str(src).strip()]:
            # An option without sources disables the self-selection gate entirely: the adjudicator
            # cannot be "in" an empty set, so choosing their own approach would not require
            # self_selection_note. The render would show only a "—", and weeks later nobody could
            # reconstruct who proposed it.
            errors.append(
                f"{where} option {option.get('id')}: missing sources. Every option must state who proposed it — the self-selection gate and later tracing both rely on it"
            )
        unknown = {str(src) for src in (option.get("sources") or [])} - all_sources
        if unknown and all_sources:
            errors.append(
                f"{where} option {option.get('id')}: sources {sorted(unknown)} are not in this round's sources {sorted(all_sources)}"
            )

    chosen_sources = {str(src) for src in (chosen.get("sources") or []) if str(src).strip()}
    # Empty sources must **not** be treated as "the adjudicator is not in it" and skip the
    # self-selection gate: that is exactly the cheapest way to disguise self-selection as not having
    # happened. When you do not know who proposed it, assume the worst case.
    if len(survivors) >= 2 and (not chosen_sources or adjudicator in chosen_sources):
        if not dispute.get("self_selection_note"):
            errors.append(
                f"{where}: adjudicator {adjudicator} chose an option they proposed and must write self_selection_note"
                " — name the concrete mechanism on which the simpler rival loses, not what is good about their own approach"
            )

    if not dispute.get("simpler_absent_check"):
        errors.append(
            f"{where}: missing simpler_absent_check"
            " — an option nobody proposed cannot win; when all candidates are non-simple you must first answer whether"
            " there is a simpler way that just nobody proposed: if you think of one add it to the candidate set, if not write \"Checked, no simpler form\""
        )
    return errors


def _source_list_errors(value, where: str) -> list[str]:
    """sources must be an array of strings.

    Fed straight into `set()` without validation: `"sources": 1` raises TypeError (not iterable),
    `[["x"]]` raises TypeError (unhashable); both escape the `LedgerError` contract as a traceback,
    and `record` / `next` only catch LedgerError.
    """

    if value is None:
        return []
    if isinstance(value, str) or not isinstance(value, (list, tuple)):
        return [f"{where}: sources must be an array of strings, got {value!r}"]
    bad = [item for item in value if not isinstance(item, str)]
    return [f"{where}: sources contains non-string items {bad!r}"] if bad else []


def validate(ledger: dict) -> list[str]:
    errors: list[str] = []
    if not ledger.get("goal"):
        errors.append("missing goal: every ranking is subordinate to it; without it there is nothing to adjudicate against")

    errors.extend(_source_list_errors(ledger.get("sources"), "top level"))
    for index, dispute in enumerate(ledger.get("disputes") or []):
        if not isinstance(dispute, dict):
            continue
        for option in dispute.get("options") or []:
            if isinstance(option, dict):
                errors.extend(
                    _source_list_errors(
                        option.get("sources"),
                        f"dispute[{index}] option {option.get('id')}",
                    )
                )
    if errors:
        # Stop when the shape does not hold: every later step does set operations on sources.
        return errors

    all_sources = set(ledger.get("sources") or [])
    if not all_sources:
        errors.append("missing sources: without knowing this round's participants, agreement and self-selection cannot be judged")
    adjudicator = ledger.get("adjudicator") or ""
    if not adjudicator:
        errors.append("missing adjudicator: the adjudicator is necessarily one of the participants, and that must be visible")
    elif all_sources and adjudicator not in all_sources:
        # Checking non-empty is not enough: an identity outside sources matches no option's sources,
        # so the self-selection gate is bypassed entirely — a self-selected winner no longer has to
        # write a failure mechanism.
        errors.append(
            f"adjudicator `{adjudicator}` is not in this round's sources {sorted(all_sources)}. "
            "The adjudicator is necessarily one of the agents in the fan-out; an outside identity disables the self-selection check"
        )
    disputes = ledger.get("disputes")
    if not isinstance(disputes, list) or not disputes:
        errors.append("missing disputes")
        return errors
    seen: set[str] = set()
    for index, dispute in enumerate(disputes):
        if not isinstance(dispute, dict):
            errors.append(f"dispute[{index}]: must be an object")
            continue
        did = dispute.get("id")
        if did in seen:
            errors.append(f"duplicate dispute id: {did}")
        if did:
            seen.add(did)
        errors.extend(_dispute_errors(dispute, index))
        # Skip the semantic checks only for "options contains non-object entries" — they assume every
        # option is an object. Other dispute-level errors do not affect _consensus_errors' premise;
        # skipping on them would only under-report.
        if all(isinstance(o, dict) for o in (dispute.get("options") or [])):
            errors.extend(_consensus_errors(dispute, adjudicator, all_sources))

    proposing = {
        str(src)
        for dispute in disputes
        if isinstance(dispute, dict)
        for option in (dispute.get("options") or [])
        if isinstance(option, dict)
        for src in (option.get("sources") or [])
    }
    deferred = {str(src) for src in (ledger.get("deferred_sources") or [])}
    unknown_deferred = deferred - all_sources
    if unknown_deferred:
        errors.append(f"deferred_sources {sorted(unknown_deferred)} are not in sources")
    silent = all_sources - proposing - deferred
    if silent:
        # A participant with no approach in the record usually means their reply was not normalized in,
        # not that they really abstained. But there is a legitimate shape too: the only dispute they
        # had an opinion on was handed back to the orchestrator per §3 a0 and kept out of the ledger.
        #
        # The hint must **not** be "remove them from sources". sources is the denominator of universal;
        # deleting a real participant launders "2 of 3 recommended" into "everyone gave the same
        # proposal" — exactly what the universal criterion exists to prevent, only now induced by the
        # validator's own hint.
        errors.append(
            f"participants {sorted(silent)} have no corresponding option. Every submitted approach must become an option"
            " — dropping it drops a future fallback. If they actually support someone else's approach, merge them into that option's "
            "sources; if their only dispute was handed back to the orchestrator, write them into the top-level `deferred_sources`. "
            "**Do not** delete them from sources: that would shrink the denominator of universal"
        )
    return errors
