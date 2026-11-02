"""Observe model labels only on a provider's identity-matched native surface."""

import re


def identity(state, selector, pane=None):
    matches = [
        a
        for a in state["agents"]
        if str(a["id"]) == str(selector) or a["name"] == str(selector)
    ]
    if len(matches) != 1:
        raise ValueError(f"invalid agent selector: {selector}")
    agent = matches[0]
    model = "unknown"
    if pane is not None:
        runtime = pane.get("runtime", {})
        launch = agent.get("runtime_identity", {}).get("launch_id")
        if (
            pane.get("agent_id") != agent["id"]
            or not launch
            or runtime.get("launch_id") != launch
        ):
            raise ValueError("invalid pane identity: agent/launch mismatch")
        if runtime.get("session_verified") is True:
            tail = pane.get("text", "").splitlines()[-8:]
            patterns = {
                "codex": r"^\s{0,4}(GPT-[A-Za-z0-9.\-]+(?: (?:xhigh|high|medium|low|minimal|none))?) · ",
                "claude_code": r"^\s{0,4}\[([^\]\r\n]{1,80})\] *│",
            }
            labels = {
                m.group(1)
                for line in tail
                if (m := re.search(patterns.get(agent["provider"], r"(?!)"), line))
            }
            if len(labels) == 1:
                model = labels.pop()
    return dict(
        id=str(agent["id"]), name=agent["name"], provider=agent["provider"], model=model
    )


def envelope(value):
    if (
        not isinstance(value, dict)
        or value.get("ok") is not True
        or not isinstance(value.get("result"), dict)
    ):
        raise ValueError("invalid Bus response: expected ok:true result")
    return value["result"]
