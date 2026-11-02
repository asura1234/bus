# Codex 0.164.0 multiline composer

Captured on macOS from real `/opt/homebrew/bin/codex` in a private scratch Bus at
`431b0de0a07c297e50f2811753b3b4005c16ebcb`, steering into foreground shell work.
Pane: 95 columns, 44 rows. Before cursor: (2,40). Clipped cursor: (56,40).
`review-reply.txt` is the unchanged 823-byte incident payload.
`before.txt` and `clipped.txt` are actual `agent observe --cells` text projections.
Codex hides the first six logical lines and adds a right-aligned up-arrow; the
old extractor returns 678 bytes / 496 characters / 25 lines. An unseen prefix
cannot be owned from this suffix. Full native-editor Verify is required before Enter.
Unmodified provider PTY input/output were recorded by a transparent proxy before
forwarding. Full cells, raw bytes, cursor, identities, and submission refusal
are archived under the main checkout's `temp/codex-composer/reproduction/` and
copied into the PR evidence directory before handoff.

`verified-marker-wrapped.txt` is the actual post-Verify redraw at Bus head
`7fa13c3ab54d58876c18b066da45d573a16ac5d1`, again in the 95 by 44 pane.
The native helper attested all 823 bytes, then Codex painted the 42-byte receipt
across rows 39 and 40 after the PID separator; its cursor is (22,40). The exact
frame, raw PTY, and successful native Verify acknowledgment are archived in
`temp/pr-294/acceptance-7fa13c3a-failed/` in the main checkout. This run sent no
Enter because the original marker reader required contiguous rendered text.
The regression binds that captured token, rejects altered or nonterminal tokens,
and replays the captured wrap in the native submission harness.
