pub(crate) const TAG: &str = "[needs_developer_attention]";
pub(crate) const WAITING_TAGS: [&str; 2] = ["[waiting-on-you]", "[waiting_on_you]"];

pub(crate) fn text(message: &str) -> Option<&str> {
    message.trim_start().strip_prefix(TAG).map(str::trim_start)
}
