//! Loopback-only, read-only view of persisted delivery records.
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::messaging::storage::state_store::JsonStore;

mod clock;
mod latency;
mod launch_metrics;
mod metrics;
mod service;

const PAGE: &str = include_str!("page.html");
const LATENCY_PAGE: &str = include_str!("latency_page.html");
const MAX_HEADERS: usize = 8192;
const REQUEST_DEADLINE: Duration = Duration::from_secs(1);

pub(crate) fn run(root: &Path) -> io::Result<()> {
    serve(root)
}

fn serve(root: &Path) -> io::Result<()> {
    let server = match service::start(root)? {
        service::Started::Existing(endpoint) => {
            writeln!(io::stdout().lock(), "{}", endpoint.url)?;
            io::stdout().flush()?;
            return Ok(());
        }
        service::Started::Owned(server) => server,
    };
    let authority = server.listener.local_addr()?.to_string();
    writeln!(io::stdout().lock(), "{}", server.endpoint.url)?;
    io::stdout().flush()?;
    server.listener.set_nonblocking(true)?;
    serve_connections(root, server, authority, std::net::TcpListener::accept)
}

fn serve_connections(
    root: &Path,
    mut server: service::Server,
    authority: String,
    mut accept: impl FnMut(&std::net::TcpListener) -> io::Result<(TcpStream, std::net::SocketAddr)>,
) -> io::Result<()> {
    loop {
        if server.retire_if_replaced(root)? {
            return Ok(());
        }
        match accept(&server.listener) {
            Ok((stream, _)) => dispatch_connection(
                stream,
                root.to_owned(),
                authority.clone(),
                server.endpoint.token.clone(),
            ),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                log_http_error(&error);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

pub(crate) use service::open_in_browser;

struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}
impl Response {
    fn empty(status: &'static str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
        }
    }
}

fn response_for(root: &Path, method: &str, path: &str) -> Response {
    if method != "GET" {
        return Response::empty("405 Method Not Allowed");
    }
    match path {
        "/" => Response {
            status: "200 OK",
            content_type: "text/html; charset=utf-8",
            body: PAGE.as_bytes().to_vec(),
        },
        "/latency" => Response {
            status: "200 OK",
            content_type: "text/html; charset=utf-8",
            body: LATENCY_PAGE.as_bytes().to_vec(),
        },
        "/latency/data" => {
            let now = crate::utils::time::now_ms();
            let metrics = match JsonStore::new(root.join("state.json")).load() {
                Ok(state) => state.map(|state| latency::snapshot(&state, latency::local_hour)),
                Err(error) => {
                    tracing::warn!(event = "bus.dashboard.latency_read_failed", %error);
                    None
                }
            };
            let hour = latency::local_hour(now);
            let offset = hour
                .label
                .rsplit_once(' ')
                .map_or("unknown offset", |(_, offset)| offset);
            data_envelope(serde_json::json!(metrics), None, now, offset)
        }
        "/data" => {
            let now = crate::utils::time::now_ms();
            let state = match JsonStore::new(root.join("state.json")).load() {
                Ok(state) => state,
                Err(error) => {
                    tracing::warn!(event = "bus.dashboard.read_failed", %error);
                    None
                }
            };
            let metrics = state
                .as_ref()
                .map(|state| metrics::snapshot(state, metrics::local_hour));
            let launches = state
                .as_ref()
                .map(|state| launch_metrics::snapshot(state, metrics::local_hour));
            let hour = metrics::local_hour(now);
            let offset = hour
                .label
                .rsplit_once(' ')
                .map_or("unknown offset", |(_, offset)| offset);
            data_envelope(
                serde_json::json!(metrics),
                Some(serde_json::json!(launches)),
                now,
                offset,
            )
        }
        _ => Response::empty("404 Not Found"),
    }
}

fn data_envelope(
    metrics: serde_json::Value,
    launches: Option<serde_json::Value>,
    now: u64,
    offset: &str,
) -> Response {
    let zone = std::env::var("TZ")
        .ok()
        .filter(|zone| !zone.is_empty())
        .or_else(|| {
            std::fs::read_link("/etc/localtime").ok().and_then(|path| {
                path.to_str()?
                    .split_once("zoneinfo/")
                    .map(|(_, zone)| zone.to_owned())
            })
        });
    let timezone = format!(
        "Server local time: {} ({offset})",
        zone.as_deref().unwrap_or("system timezone")
    );
    let mut data = serde_json::json!({
        "generated_at_ms": now,
        "timezone": timezone,
        "metrics": metrics,
    });
    if let (Some(object), Some(launches)) = (data.as_object_mut(), launches) {
        object.insert("launches".to_owned(), launches);
    }
    Response {
        status: "200 OK",
        content_type: "application/json; charset=utf-8",
        body: data.to_string().into_bytes(),
    }
}

fn read_headers(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + REQUEST_DEADLINE;
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    while bytes.len() <= MAX_HEADERS {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "HTTP header deadline"))?;
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Incomplete HTTP headers",
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") && bytes.len() <= MAX_HEADERS {
            return Ok(bytes);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "HTTP headers too large",
    ))
}

fn http_response(root: &Path, headers: &[u8], authority: &str, token: &str) -> Response {
    let Ok(headers) = std::str::from_utf8(headers) else {
        return Response::empty("400 Bad Request");
    };
    let mut lines = headers.split("\r\n");
    let request: Vec<_> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if request.len() != 3 || !matches!(request[2], "HTTP/1.1" | "HTTP/1.0") {
        return Response::empty("400 Bad Request");
    }
    let mut hosts = 0;
    for line in lines.take_while(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            return Response::empty("400 Bad Request");
        };
        if key.eq_ignore_ascii_case("host") {
            hosts += 1;
            if value.trim() != authority {
                return Response::empty("403 Forbidden");
            }
        }
        if key.eq_ignore_ascii_case("origin") && value.trim() != format!("http://{authority}") {
            return Response::empty("403 Forbidden");
        }
    }
    if hosts != 1 {
        return Response::empty("403 Forbidden");
    }
    if request[0] == "GET" && request[1] == "/health" {
        return Response {
            status: "200 OK",
            content_type: "application/json",
            body: serde_json::json!({"token":token,"build":env!("BUS_BUILD_SHA")})
                .to_string()
                .into_bytes(),
        };
    }
    response_for(root, request[0], request[1])
}

fn serve_connection(
    stream: &mut TcpStream,
    root: &Path,
    authority: &str,
    token: &str,
) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    let headers = read_headers(stream)?;
    let response = http_response(root, &headers, authority, token);
    stream.set_write_timeout(Some(REQUEST_DEADLINE))?;
    write!(stream, "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'\r\n\r\n", response.status, response.content_type, response.body.len())?;
    stream.write_all(&response.body)
}

#[cfg(test)]
#[path = "tests/page_render_test.rs"]
mod page_render_tests;
#[cfg(test)]
#[path = "tests/server_test.rs"]
mod tests;

static ACTIVE_CONNECTIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct ConnectionPermit;
impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        ACTIVE_CONNECTIONS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}
fn dispatch_connection(
    mut stream: TcpStream,
    root: std::path::PathBuf,
    authority: String,
    token: String,
) {
    use std::sync::atomic::Ordering;
    if ACTIVE_CONNECTIONS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < 16).then_some(count + 1)
        })
        .is_err()
    {
        tracing::debug!(event = "bus.dashboard.connection_limit");
        return;
    }
    let permit = ConnectionPermit;
    if let Err(error) = std::thread::Builder::new()
        .name("bus-dashboard-http".into())
        .spawn(move || {
            let _permit = permit;
            if let Err(error) = serve_connection(&mut stream, &root, &authority, &token) {
                log_http_error(&error);
            }
        })
    {
        log_http_error(&error);
    }
}
fn log_http_error(error: &io::Error) {
    if matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    ) {
        tracing::debug!(event = "bus.dashboard.connection_closed", %error);
    } else {
        tracing::warn!(event = "bus.dashboard.http_failed", %error);
    }
}

#[cfg(test)]
#[path = "tests/fixtures_test.rs"]
mod fixtures;

#[cfg(test)]
#[path = "tests/launch_metrics_test.rs"]
mod launch_metrics_tests;

#[cfg(test)]
#[path = "tests/latency_test.rs"]
mod latency_tests;
