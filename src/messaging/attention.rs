pub(crate) const TAG: &str = "[needs_developer_attention]";

pub(crate) fn text(message: &str) -> Option<&str> {
    message.trim_start().strip_prefix(TAG).map(str::trim_start)
}
