"""Lane F owns Grok-specific real acceptance; G0 never reports a pass."""
COMMAND = "grok"


def case_support(case):
    return "Grok acceptance is not implemented (G0; lane F)"


def dialog_prompt(token):
    raise NotImplementedError("Grok dialog probe is not implemented (G0; lane F)")


def run_case(run, case, row):
    raise NotImplementedError("Grok acceptance is not implemented (G0; lane F)")
