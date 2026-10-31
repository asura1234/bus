use super::*;

fn inspector(kind: AgentKind) -> PreservingComposer {
    PreservingComposer::new(
        kind,
        "from bus".into(),
        std::path::PathBuf::from("unused-policy-test"),
        vec![
            Bytes::from_static(b"\x13"),
            Bytes::from_static(b"\x07"),
            Bytes::from_static(b"\x7f"),
        ],
    )
}

fn screen(kind: AgentKind, draft: &str) -> String {
    match kind {
        AgentKind::Claude => format!("────────────────\n❯ {draft}\n────────────────"),
        AgentKind::Codex => format!("› {draft}\n\nGPT-6.1 · /workspace\n? for shortcuts"),
        AgentKind::Cursor => format!(" > {draft}\n / commands · @ files"),
        _ => unreachable!(),
    }
}

#[test]
fn each_supported_provider_selects_its_verified_draft_preservation_path() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let observer = inspector(kind);
        assert!(observer.draft_preservation(&screen(kind, "")).is_none());
        let plain = observer
            .draft_preservation(&screen(kind, "unsent plain text"))
            .unwrap();
        match (kind, plain) {
            (
                AgentKind::Claude,
                DraftPreservation::Native {
                    key, visible_draft, ..
                },
            ) => {
                assert_eq!(key.as_ref(), b"\x13");
                assert_eq!(visible_draft, "unsent plain text");
            }
            (
                AgentKind::Codex | AgentKind::Cursor,
                DraftPreservation::Editor { key, backspace, .. },
            ) => {
                assert_eq!(key.as_ref(), b"\x07");
                assert_eq!(backspace.as_ref(), b"\x7f");
            }
            _ => panic!("wrong preservation path for {kind:?}"),
        }
        for opaque in [
            "[Image #138] draft",
            "[Pasted text #147 +20 lines]",
            "[Pasted Content 2048 chars]",
        ] {
            let chosen = observer.draft_preservation(&screen(kind, opaque));
            assert_eq!(
                chosen.is_some(),
                kind == AgentKind::Claude,
                "{kind:?}: {opaque}"
            );
            if let Some(chosen) = chosen {
                assert!(matches!(chosen, DraftPreservation::Native { .. }));
            }
        }
    }
}

#[test]
fn claude_accepted_suggestion_with_cursor_at_start_is_preserved() {
    let observer = inspector(AgentKind::Claude);
    let accepted = screen(AgentKind::Claude, "Try \"fix typecheck errors\"");
    // Tab accepts the native suggestion into a real draft; Ctrl+A then moves
    // its cursor back to the first content cell. Literal text reaches the
    // same state and must receive the same preservation treatment.
    assert_eq!(
        observer.observe_with_cursor(&accepted, Some((2, 1))),
        InputObservation::Other
    );
    assert!(observer
        .draft_preservation_with_cursor(&accepted, Some((2, 1)))
        .is_some());
    assert_eq!(
        observer.observe_with_launch(&accepted, Some((2, 1)), true),
        InputObservation::Empty
    );
    assert!(observer
        .draft_preservation_since_launch(&accepted, Some((2, 1)), true)
        .is_none());
}

#[test]
fn an_existing_native_stash_and_an_unconfigured_binding_are_left_untouched() {
    let observer = inspector(AgentKind::Claude);
    let occupied_slot = format!(
        "{}\n› stashed",
        screen(AgentKind::Claude, "new developer draft")
    );
    assert!(observer.draft_preservation(&occupied_slot).is_none());
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let observer =
            PreservingComposer::new(kind, "from bus".into(), "unused".into(), Vec::new());
        assert!(observer
            .draft_preservation(&screen(kind, "developer draft"))
            .is_none());
    }
}

#[test]
fn native_restore_requires_the_same_rich_draft_and_a_consumed_stash_slot() {
    let observer = inspector(AgentKind::Claude);
    let saved = screen(
        AgentKind::Claude,
        "[Image #138] [Pasted text #147 +20 lines]",
    );
    assert!(observer.draft_preservation(&saved).is_some());
    assert!(observer.native_draft_restored(&saved));
    assert!(
        !observer.native_draft_restored(&screen(AgentKind::Claude, "[Image #138] developer edit"))
    );
    assert!(!observer.native_draft_restored(&format!("{saved}\n› stashed")));
    assert!(!observer.native_draft_restored(&screen(AgentKind::Claude, "")));
}

#[test]
fn only_a_marker_at_the_live_composer_end_authorizes_sentinel_deletion() {
    let marker = "__BUS_DRAFT_END_cafe__";
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let observer = inspector(kind);
        assert!(
            observer.restoration_marker_visible(&screen(kind, &format!("draft {marker}")), marker)
        );
        assert!(!observer.restoration_marker_visible(
            &screen(kind, &format!("{marker} developer edit")),
            marker
        ));
        assert!(!observer.restoration_marker_visible(
            &format!("old transcript {marker}\n{}", screen(kind, "draft")),
            marker
        ));
    }
}

#[test]
fn invisible_whitespace_is_preserved_but_a_footer_cursor_is_not_a_draft() {
    for kind in [AgentKind::Claude, AgentKind::Codex, AgentKind::Cursor] {
        let row = u16::from(kind == AgentKind::Claude);
        let observer = inspector(kind);
        let empty = screen(kind, "");
        assert!(observer
            .draft_preservation_with_cursor(&empty, Some((5, row)))
            .is_some());
        assert!(observer
            .draft_preservation_with_cursor(&empty, Some((2, row)))
            .is_none());
        assert!(!invisible_content(kind, &empty, Some((30, row + 2))));
    }
}
