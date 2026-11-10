pub(crate) mod activity;
pub(crate) mod hooks;
pub(crate) mod launch;
pub(crate) mod resume;
pub(crate) mod system_prompt;
pub(crate) mod wire;

pub(crate) fn quota_exhausted(error: &str) -> bool {
    error
        .split_once(':')
        .is_some_and(|(name, _)| name.eq_ignore_ascii_case("APIProviderQuotaExhaustedError"))
}
