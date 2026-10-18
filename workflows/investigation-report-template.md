# Investigation report: <bug title>

This report records observed facts and the evidence behind them. It does not
propose fixes, judge priority, or plan verification. Every claim cites its
evidence.

Source: <link or path to the original bug report>
Investigated at commit: <sha>

## 1. Symptoms

- <one observed symptom per bullet: what happens, and what should happen instead>

## 2. User-experience impact

- <one negative effect on the user's experience per bullet>

## 3. Reproduction

Reliability: <always | intermittent: N of M runs | not reproduced>

Steps:

1. <step>

Reproduction artifacts:

- UI run report and logs: <path, or "none">
- Failing tests written: <test name, file path, observed failure output, or "none">

## 4. Evidence

| # | Source (repro run, attached log, code read) | What it shows | Path / file:line |
|---|---|---|---|
| 1 | <source> | <observation> | <location> |

## 5. Root-cause certainty

Status: <pinned | suspected | unknown>

- What the evidence establishes: <chain from symptom to code location, each link citing an Evidence #>
- What the evidence rules out: <each alternative and the Evidence # that excludes it>
- What is missing to be sure: <evidence not yet available>

"Pinned" requires an unbroken chain from symptom to code location, with the
alternatives ruled out. If any link is inferred rather than observed, the
status is "suspected".
