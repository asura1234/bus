use super::*;

#[test]
fn waiting_tags_must_open_a_prose_line() {
    for message in [
        "Never to the developer, MASTER or Feishu. No `[waiting-on-you]`.",
        "Please quote [waiting-on-you] Approve access",
        "`[waiting-on-you]` Approve access",
        "**No [waiting_on_you]** Approve access",
        "```text\n[waiting-on-you] Approve access\n```",
        "~~~\n**[waiting_on_you]** Approve access\n~~~",
        "````md\n```\n[waiting-on-you] Approve access\n````",
        "    [waiting-on-you] Approve access",
        "An inline `code span\n[waiting-on-you] Approve access\nends here`.",
    ] {
        assert_eq!(waiting_tag_action(message), None, "{message}");
        assert_eq!(waiting_action(message), None, "{message}");
    }
}

#[test]
fn waiting_tags_allow_line_prefix_markdown_and_case_folding() {
    for message in [
        "[waiting-on-you] Approve access",
        "  [WAITING_ON_YOU]: Approve access",
        "- **[waiting-on-you]**: Approve access",
        "* [waiting_on_you] Approve access",
        "__[waiting-on-you]__ Approve access",
        "1. **[waiting-on-you]** Approve access",
        "`example`\n[waiting-on-you] Approve access",
        "```\n[waiting-on-you] ignored\n```\n[waiting-on-you] Approve access",
        "~~~\n[waiting-on-you] ignored\n~~~\n[waiting-on-you] Approve access",
        "说明。\r\n[waiting-on-you] Approve access",
    ] {
        assert_eq!(
            waiting_tag_action(message),
            Some("Approve access"),
            "{message}"
        );
        assert_eq!(waiting_action(message), Some("Approve access"), "{message}");
    }
}

#[test]
fn waiting_action_requires_meaningful_text_and_can_use_following_lines() {
    for message in [
        "[waiting-on-you]",
        "[waiting-on-you] `.`,\n...\n—！",
        "**[waiting_on_you]**:\n \n---",
    ] {
        assert_eq!(waiting_tag_action(message), None, "{message}");
        assert_eq!(waiting_action(message), None, "{message}");
    }
    for message in [
        "[waiting-on-you]\n\nUnlock the phone",
        "[waiting-on-you] .\n...\nUnlock the phone",
        "**[waiting_on_you]**:\n\nUnlock the phone",
    ] {
        assert_eq!(
            waiting_tag_action(message),
            Some("Unlock the phone"),
            "{message}"
        );
    }
    let choices = "Pick one:\n- **B (recommended):** Keep the current API.\n- **A:** Change it.";
    assert_eq!(
        waiting_tag_action(&format!("[waiting-on-you] {choices}")),
        Some(choices)
    );
}

#[test]
fn waiting_action_body_bounds_context_without_shortening_the_action_line() {
    let first = "界".repeat(4200);
    assert_eq!(plain_action(&first), first);
    assert_eq!(
        plain_action(&format!("{first}\nMore")),
        format!("{first}\n…")
    );
    let many = format!(
        "Pick one:\n{}",
        (1..20)
            .map(|i| format!("- Choice {i}\n"))
            .collect::<String>()
    );
    let body = plain_action(&many);
    assert!(body.contains("- Choice 11\n…"), "{body}");
    assert!(!body.contains("Choice 12"));
    assert_eq!(
        plain_action("Pick one:\r\n- B\u{2028}- A\rEnd\tplease\u{7}"),
        "Pick one:\n- B\n- A\nEnd please"
    );
    assert_eq!(
        plain_action(&format!("Pick one:\n{}", "界".repeat(4100))),
        "Pick one:\n…"
    );
}
