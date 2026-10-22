# Grok dialog fixtures — SYNTHETIC

These fixtures describe the supported synthetic card contract.
They are parser hypotheses, not captures of authenticated Grok Build 1.0.5 cards.
The exact binary-matching local guide `~/.grok/docs/user-guide/03-keyboard-shortcuts.md`
and the historical 0.2.101 manifest inform the keys and gutter shapes. Permission, trust,
question, narrow/wrapped and parked-focus model cards remain uncaptured.

Supported synthetic contract:

- A live card starts at a `◆` heading and ends at its contextual key footer.
  `┃` rows carry a unique `1`–`9`, `a`–`f` or `z` selector and `(○)/(●)`
  or `[ ]/[✓]/[✔]/[x]` state. API option numbers are contiguous screen rows.
- `❯` or a unique highlighted row supplies focus; a unique filled radio is
  accepted only for radio cards. Checkbox check state never supplies focus.
- Complete permission/trust radio choices use arrows and Enter; their numeric
  rows must match the visible `1/N:select` range. Radio questions reject selector
  gaps. Question answer rows use stable documented shortcuts, including `z`,
  so hidden rows cannot change an arrow distance or selection-independent ID.
  A separate toggle/edit action uses Space. Enter advances/submits; question
  navigation uses Left/Right and is exposed as separate options.
- A `z` row is an editable Question only with `Enter:submit` plus `Esc:back`.
  It has no choice options; its current text contributes to the digest.
  The skip contract is `shift+x`: shared runtime must leave editing with Esc,
  then send Shift+X to dismiss. Esc alone never skips or approves.
- Scope, complete labels/check state, focused row, active question, card styles
  and input all contribute to the existing digest. Checkbox state/input edits
  preserve dialog identity but invalidate a previously observed digest.
- `Tab/Space: question` or `Tab/Space: permission` means parked focus: the card
  stays blocked and exposes no actionable choice. Return focus manually first.
- Unknown, incomplete, duplicate-selector or ambiguously focused cards fail closed.

`permission.txt`, `trust.txt`, `question.txt`, `free-text.txt`,
`parked-focus.txt` and `wrapped.txt` are all SYNTHETIC. Real authenticated card
captures, keyboard behavior and fingerprinted PTY acceptance remain pending.
