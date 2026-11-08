//! Compaction banners on a provider pane.
//!
//! Grok and Kimi do not emit Claude's SessionStart `source: "compact"` hook.
//! Their TUIs print a short status line for one compaction. Bus already reads
//! that pane. A banner that stays on screen, including after a resize wraps
//! it, is still the same episode.

/// Status lines longer than this are transcript prose, not the banner.
const MAX_BANNER_CHARS: usize = 160;

const GROK_PHRASES: &[&str] = &["Auto-compacting conversation", "Conversation compacted"];

const KIMI_PHRASES: &[&str] = &[
    "Compacting context",
    "Context auto-compacted",
    "Context compacted",
];

/// Whether `screen` is showing one compaction banner for this provider label.
///
/// Both the in-progress line and the completion line are the same episode.
/// `None` and every provider other than Grok and Kimi never match, so Claude's
/// hook path cannot also count the words if they appear on its pane.
pub(crate) fn visible(agent: Option<&str>, screen: &str) -> bool {
    let phrases = match agent {
        Some("grok") => GROK_PHRASES,
        Some("kimi") => KIMI_PHRASES,
        _ => return false,
    };
    let lines: Vec<String> = screen
        .lines()
        .map(|line| collapse_whitespace(line.trim()))
        .filter(|line| !line.is_empty())
        .collect();
    if lines.iter().any(|line| status_line(line, phrases)) {
        return true;
    }
    // A resize can split the status line on a word or mid-word. The mid-word
    // break has no space; the word break does. Only one short pair is joined,
    // so a long transcript still cannot match.
    lines.windows(2).any(|pair| {
        let mid_word = format!("{}{}", pair[0], pair[1]);
        let word = format!("{} {}", pair[0], pair[1]);
        status_line(&mid_word, phrases) || status_line(&word, phrases)
    })
}

fn short_banner(line: &str) -> bool {
    line.chars().count() <= MAX_BANNER_CHARS
}

/// The phrase is the whole status line, or its prefix plus a terminator.
/// A period ends the completion line. ` (` starts the progress suffix.
/// A later sentence that merely contains the phrase does not match.
fn status_line(line: &str, phrases: &[&str]) -> bool {
    short_banner(line)
        && phrases.iter().any(|phrase| {
            let Some(rest) = line.strip_prefix(phrase) else {
                return false;
            };
            rest.is_empty() || rest == "." || rest.starts_with(" (")
        })
}

fn collapse_whitespace(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::visible;

    #[test]
    fn compaction_banner_grok_in_progress_matches() {
        assert!(visible(
            Some("grok"),
            "Auto-compacting conversation (82% full)..."
        ));
        assert!(visible(
            Some("grok"),
            "Auto-compacting   conversation (82% full)..."
        ));
    }

    #[test]
    fn compaction_banner_grok_completed_matches() {
        assert!(visible(Some("grok"), "Conversation compacted."));
    }

    #[test]
    fn compaction_banner_grok_wrapped_banner_still_matches() {
        assert!(visible(
            Some("grok"),
            "Auto-compacting\nconversation (82% full)..."
        ));
    }

    #[test]
    fn compaction_banner_grok_ignores_failure_prose_and_idle() {
        assert!(!visible(Some("grok"), "Auto-compact failed"));
        assert!(!visible(Some("grok"), "Auto-compact cancelled"));
        assert!(!visible(Some("grok"), "ready\nask anything"));
        let prose = format!("Auto-compacting conversation {}", "x".repeat(MAX_PROSE));
        assert!(
            !visible(Some("grok"), &prose),
            "a long transcript line is not the status banner"
        );
    }

    #[test]
    fn compaction_banner_kimi_status_phrases_match() {
        assert!(visible(Some("kimi"), "Compacting context"));
        assert!(visible(Some("kimi"), "Context compacted"));
        assert!(visible(Some("kimi"), "Context auto-compacted"));
        assert!(
            !visible(Some("kimi"), "Compacting conversation context"),
            "the longer transcript chunk is not the status banner"
        );
    }

    #[test]
    fn compaction_banner_other_agents_never_match() {
        for agent in [None, Some("claude"), Some("codex"), Some("cursor")] {
            assert!(!visible(agent, "Auto-compacting conversation"));
            assert!(!visible(agent, "Compacting context"));
        }
    }

    const MAX_PROSE: usize = 180;
}
