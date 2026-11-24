use super::ProviderKind;
use std::{
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    sync::mpsc,
    time::{Duration, Instant},
};

const DEFAULT_URL: &str = "http://127.0.0.1:3425";
const ARGON_BASE_URL: &str = "CLAUDE_ARGON_BASE_URL";
const CONNECT_BUDGET: Duration = Duration::from_millis(750);

pub(crate) fn preflight_gateway(provider: ProviderKind) -> Result<(), String> {
    let Some(url) = gateway_url(provider) else {
        return Ok(());
    };
    let endpoint = GatewayEndpoint::parse(&url).ok_or_else(|| {
        format!(
            "Invalid Magpie gateway URL in {ARGON_BASE_URL}. Set a valid HTTP or HTTPS URL, start Magpie with `magpie serve`, then retry."
        )
    })?;
    if endpoint.can_connect() {
        return Ok(());
    }
    // The authority may contain credentials, and the path/query may carry
    // tokens. The origin identifies the unreachable listener without either.
    Err(format!(
        "Cannot connect to the Magpie gateway at {}. Start Magpie with `magpie serve` and retry.",
        endpoint.origin
    ))
}

fn gateway_url(provider: ProviderKind) -> Option<String> {
    let argon = match provider {
        ProviderKind::ClaudeArgon => true,
        ProviderKind::ClaudeGpt => false,
        _ => return None,
    };
    #[cfg(test)]
    if let Some(url) = super::gateway_test_support::URLS.with_borrow(|urls| {
        urls.as_ref().map(|urls| {
            urls.argon_override
                .as_ref()
                .filter(|url| argon && !url.is_empty())
                .unwrap_or(&urls.default)
                .clone()
        })
    }) {
        return Some(url);
    }
    let configured = argon.then(|| std::env::var_os(ARGON_BASE_URL)).flatten();
    Some(
        configured
            .filter(|url| !url.is_empty())
            .map(|url| url.to_string_lossy().into_owned())
            .unwrap_or_else(|| DEFAULT_URL.into()),
    )
}

struct GatewayEndpoint {
    origin: String,
    host: String,
    port: u16,
}

impl GatewayEndpoint {
    fn parse(url: &str) -> Option<Self> {
        let uri: http::Uri = url.parse().ok()?;
        let scheme = uri.scheme_str()?;
        let default_port = match scheme {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        let authority = uri.authority()?;
        let host = authority.host();
        let host_port = authority.as_str().rsplit('@').next()?;
        let port = match host_port.strip_prefix(host)? {
            "" => default_port,
            explicit => explicit.strip_prefix(':')?.parse().ok()?,
        };
        (!host.is_empty() && port != 0).then(|| Self {
            origin: format!("{scheme}://{host_port}"),
            host: host.trim_start_matches('[').trim_end_matches(']').into(),
            port,
        })
    }

    fn can_connect(&self) -> bool {
        let deadline = Instant::now() + CONNECT_BUDGET;
        let addrs = self.addresses(deadline);
        // Share the budget so an unreachable IPv6 address cannot consume the
        // only chance to connect to a reachable IPv4 address, or vice versa.
        addrs.iter().enumerate().any(|(index, addr)| {
            let timeout =
                deadline.saturating_duration_since(Instant::now()) / (addrs.len() - index) as u32;
            !timeout.is_zero() && TcpStream::connect_timeout(addr, timeout).is_ok()
        })
    }

    fn addresses(&self, deadline: Instant) -> Vec<SocketAddr> {
        if let Ok(ip) = self.host.parse::<IpAddr>() {
            return vec![SocketAddr::new(ip, self.port)];
        }
        // The system resolver is blocking. Its own timeout must not extend the
        // launch budget, and it owns no Bus or terminal state after we return.
        let (host, port) = (self.host.clone(), self.port);
        let (send, receive) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("magpie-resolve".into())
            .spawn(move || {
                let addrs = (host.as_str(), port)
                    .to_socket_addrs()
                    .map(Iterator::collect)
                    .unwrap_or_default();
                let _ = send.send(addrs);
            })
            .is_err()
        {
            return Vec::new();
        }
        receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_default()
    }
}
