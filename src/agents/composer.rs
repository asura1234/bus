//! Codex placeholders shared by dialog, submission and launch detection.
pub(crate) const CODEX_EMPTY_COMPOSER_PLACEHOLDERS: [&str; 2] = [
    "Ask Codex to do anything",
    "Use /skills to list available skills",
];

pub(crate) fn is_empty_codex_composer(body: &str) -> bool {
    let body = body.trim();
    body.is_empty() || CODEX_EMPTY_COMPOSER_PLACEHOLDERS.contains(&body)
}
