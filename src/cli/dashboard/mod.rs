//! Loopback-only, read-only view of persisted delivery records.
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::messaging::storage::state_store::JsonStore;
mod metrics;

const PAGE: &str = include_str!("page.html");
const MAX_HEADERS: usize = 8192;
const REQUEST_DEADLINE: Duration = Duration::from_secs(1);

pub(super) fn run(root: &Path) -> io::Result<()> {
    let mut options = super::logging_options();
    // This reader has its own log, never an error-pipe inbox or a TUI notice.
    options.dev = false;
    crate::utils::logging::init_file_logging_at(
        crate::utils::paths::data_dir(),
        "dashboard.log",
        &options,
    );
    if let Err(error) = serve(root) {
        tracing::warn!(event = "bus.dashboard.serve_failed", %error);
    }
    Ok(())
}

fn serve(root: &Path) -> io::Result<()> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let authority = listener.local_addr()?.to_string();
    writeln!(io::stdout().lock(), "http://{authority}/")?;
    io::stdout().flush()?;
    for connection in listener.incoming() {
        let result =
            connection.and_then(|mut stream| serve_connection(&mut stream, root, &authority));
        if let Err(error) = result {
            tracing::warn!(event = "bus.dashboard.http_failed", %error);
        }
    }
    Ok(())
}

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
        "/data" => {
            let now = crate::utils::time::now_ms();
            let metrics = match JsonStore::new(root.join("state.json")).load() {
                Ok(state) => state.map(|state| metrics::snapshot(&state, metrics::local_hour)),
                Err(error) => {
                    tracing::warn!(event = "bus.dashboard.read_failed", %error);
                    None
                }
            };
            let hour = metrics::local_hour(now);
            let offset = hour
                .label
                .rsplit_once(' ')
                .map_or("unknown offset", |(_, offset)| offset);
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
            let data =
                serde_json::json!({"generated_at_ms":now,"timezone":timezone,"metrics":metrics});
            Response {
                status: "200 OK",
                content_type: "application/json; charset=utf-8",
                body: data.to_string().into_bytes(),
            }
        }
        _ => Response::empty("404 Not Found"),
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

fn http_response(root: &Path, headers: &[u8], authority: &str) -> Response {
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
    response_for(root, request[0], request[1])
}

fn serve_connection(stream: &mut TcpStream, root: &Path, authority: &str) -> io::Result<()> {
    let headers = read_headers(stream)?;
    let response = http_response(root, &headers, authority);
    stream.set_write_timeout(Some(REQUEST_DEADLINE))?;
    write!(stream, "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'\r\n\r\n", response.status, response.content_type, response.body.len())?;
    stream.write_all(&response.body)
}

#[cfg(test)]
#[path = "tests/server_test.rs"]
mod tests;
