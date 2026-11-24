use std::cell::RefCell;

thread_local! {
    pub(super) static URLS: RefCell<Option<GatewayUrls>> = const { RefCell::new(None) };
}

pub(crate) struct GatewayUrls {
    pub(super) default: String,
    pub(super) argon_override: Option<String>,
}

pub(crate) struct GatewayOverride(Option<GatewayUrls>);

impl GatewayOverride {
    pub(crate) fn set(default: String, argon_override: Option<String>) -> Self {
        Self(URLS.replace(Some(GatewayUrls {
            default,
            argon_override,
        })))
    }
}

impl Drop for GatewayOverride {
    fn drop(&mut self) {
        URLS.replace(self.0.take());
    }
}
