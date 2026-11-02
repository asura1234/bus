//! Recognize Kimi's current native allowance error without inferring a reset time.
const QUOTA_ERROR: &str = "429 You've reached your 5-hour usage limit. Your quota will reset when the current 5-hour window ends. (provider.api_error)";

pub(crate) fn is_quota_error(error: &str) -> bool {
    error == QUOTA_ERROR
}

pub(crate) fn quota_error(screen: &str) -> Option<String> {
    let lines: Vec<_> = screen.lines().collect();
    let start = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with("❯ "))
        .map_or(0, |index| index + 1);
    let current = lines[start..].join(" ");
    let current = current.split_whitespace().collect::<Vec<_>>().join(" ");
    let (_, error) = current.rsplit_once("Error: [provider.api_error] 429 ")?;
    (error.contains("You've reached your 5-hour usage limit.")
        && error.contains("Your quota will reset when the current 5-hour window ends."))
    .then(|| QUOTA_ERROR.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANNER: &str = "Error: [provider.api_error] 429 You've reached your 5-hour usage limit. Your quota will reset when the current 5-hour window ends.";

    #[test]
    fn kimi_quota_error_survives_native_word_wrapping_and_busy_footer() {
        let screen = format!(
            "{}\n│ >  │\nK3.1 Pro thinking: high [1 task running]\ncontext: 3%",
            BANNER.replace("usage limit", "usage\nlimit")
        );
        assert_eq!(quota_error(&screen).as_deref(), Some(QUOTA_ERROR));
    }

    #[test]
    fn kimi_quota_error_does_not_classify_an_ordinary_429_or_another_provider() {
        for screen in [
            "Error: [provider.api_error] 429 Too many requests. Retry after one second.",
            "Error: You've reached your monthly usage limit Request higher limits to continue using Cursor",
            "You've reached your 5-hour usage limit. Your quota will reset when the current 5-hour window ends.",
        ] {
            assert_eq!(quota_error(screen), None, "{screen}");
        }
    }

    #[test]
    fn kimi_quota_error_ignores_history_before_a_new_native_prompt() {
        let recovered =
            format!("{BANNER}\n❯ Reply after the reset\n● Success.\n│ >  │\ncontext: 3%");
        assert_eq!(quota_error(&recovered), None);
        assert_eq!(quota_error("│ >  │\ncontext: 3%"), None);
        let failed_again = format!("{recovered}\n{BANNER}");
        assert_eq!(quota_error(&failed_again).as_deref(), Some(QUOTA_ERROR));
    }
}
