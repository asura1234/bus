use std::cell::RefCell;

thread_local! {
    static URLS: RefCell<Option<GatewayUrls>> = const { RefCell::new(None) };
}

struct GatewayUrls {
    default: String,
    argon_override: Option<String>,
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

pub(super) fn override_url(argon: bool) -> Option<String> {
    URLS.with_borrow(|urls| {
        urls.as_ref().map(|urls| {
            urls.argon_override
                .as_ref()
                .filter(|url| argon && !url.is_empty())
                .unwrap_or(&urls.default)
                .clone()
        })
    })
}
