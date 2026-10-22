# Grok composer fixtures — SYNTHETIC

All three text files are synthetic normalized frames authored for lane C.
The box, `❯ Type a message...` placeholder and `Shift+Tab:mode │ Ctrl+x:shortcuts`
footer are informed by offline 1.0.5 evidence in the integrator's
`temp/grok-provider/tui-offline-builtins.ansi`; these files are not direct captures.
The version-matching keyboard guide documents Ctrl+X as the non-Kitty shortcut.

`empty.txt` represents an empty boxed composer. `draft.txt` demonstrates editable
numbered instructions. `pasted.txt` is an explicitly hypothetical collapsed chip;
its exact rendering and count semantics have no authenticated evidence.

Only a complete current composer frame with normal mode footer is readable.
A live card, parked card, modal, incomplete frame or login is unavailable. Plain
unrelated drafts return Unavailable: the shared runtime clears Other drafts,
so that observation would overwrite developer input. Only exact visible text
is an owned pending prompt.
Only CRLF line endings are normalized; indentation and visible line breaks must
match exactly. Ambiguous soft wrapping remains unavailable. A faint placeholder
is empty; the same literal text without placeholder styling is a developer draft.
A collapsed paste chip returns Unavailable: matching the hidden text by count
alone would permit Enter to submit an unrelated draft of the same length. Real
long-paste observation/ownership needs an actual capture and an attestable shape.
