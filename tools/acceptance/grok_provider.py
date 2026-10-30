"""Grok Build acceptance adapter; only operates on the caller's scratch Run.

These are authored synthetic tasks for authenticated acceptance, not claims
that Grok 1.0.5 turns/cards have been captured. Native background wake stays
pending; orchestrators use ordinary Bus sends and durable Bus notifications.
"""
COMMAND = "grok"
_CASES = {"single", "queued", "steering", "multi", "dialog", "orchestrator", "adoption", "resume"}


def case_support(case: str) -> str | None:
    if case == "background":
        return "PENDING_AUTH: native Grok background wake needs a no-input authenticated probe"
    if case not in _CASES:
        return f"unsupported Grok acceptance case: {case}"
    return None


def dialog_prompt(token: str) -> str:
    return ("Synthetic Bus dialog acceptance. Do not use skills or shell commands. "
            "Use ask_user_question once to ask 'Continue the Bus e2e check?' "
            "with exactly two options, Yes and No. Wait for the answer. "
            f"Then reply with exactly {token} and nothing else.")


def _resume(run, row):
    # Run.run_case dispatches here instead of executing case_resume's closure.
    # Keep both checks that closure normally owns.
    old = run.report.get("resume", {}).get("sessions_before", {}).get("grok")
    assert old, "no Grok session before the scratch Bus restart"
    run.round_trip("grok", row, "resume")
    assert run.sessions.get("grok") == old, "Grok did not resume the original session"

    def terminal(record):
        return ((record.get("grok") or {}).get("identity") or {}).get("terminal_id")
    before = terminal(run.report.get("setup", {}))
    after = terminal(run.report.get("resume", {}).get("ready", {}))
    assert before and after and before != after, "no proof of a new Grok terminal after restart"


def run_case(run, case: str, row: dict) -> None:
    reason = case_support(case)
    if reason:
        # Use the shared exception so this cannot become a false PASS or FAIL.
        from tools.acceptance.e2e_test import Skip
        raise Skip(reason)
    if case == "resume":
        _resume(run, row)
    elif case == "adoption":
        assert run.sessions.get("orch-grok"), "select and pass the orchestrator case before adoption"
        run.case_adoption("grok", row)
    elif case == "multi":
        raise AssertionError("multi-provider acceptance must use Run.case_multi once")
    elif case == "orchestrator":
        # The generic path uses provider case_support to omit native background.
        # Durable worker-wake and routing races have their own post-join checklist.
        run.case_orchestrator("grok", row)
        row["native_background"] = case_support("background")
    else:
        getattr(run, "case_" + case)("grok", row)
