//! Login prerequisites use the state detector's precise control-surface rule.
use super::{Dialog, DialogKind, DialogOption};

pub(super) fn parse(lines: &[&str]) -> Option<Dialog> {
    let screen = lines.join("\n");
    if !crate::agents::manifest::grok_login_required(&screen) {
        return None;
    }
    let connecting = lines
        .iter()
        .any(|line| matches!(line.trim(), "Connecting..." | "Connecting…"));
    let menu = lines
        .iter()
        .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["Login", "with", "Grok", "l"]);
    let options = if menu {
        [("Login with Grok", "l"), ("Quit", "q")]
            .into_iter()
            .enumerate()
            .map(|(index, (label, key))| DialogOption {
                number: index as u32 + 1,
                label: label.into(),
                selected: false,
                free_text: false,
                navigation_key: Some(key),
                identity_label: None,
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(Dialog {
        kind: DialogKind::Choice,
        input: None,
        text: if connecting {
            "Grok login required. Connecting to the login service..."
        } else {
            "Grok login required. Sign in to Grok to continue."
        }
        .into(),
        options,
        hint: None,
        fingerprint_context: screen,
        tabs: Vec::new(),
    })
}
