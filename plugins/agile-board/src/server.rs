use crate::{
    error::{Error, Result},
    model::{local_midnight_ms, now_epoch_ms, ViewFilter, ALL_COLUMNS},
    store,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const PAGE_HTML: &str = include_str!("page.html");

pub fn serve(
    data_dir: &Path,
    port: u16,
    run_for_secs: Option<u64>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| Error(1, format!("failed to bind 127.0.0.1:{port}: {e}")))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| Error(1, format!("failed to read bound address: {e}")))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| Error(1, format!("failed to set nonblocking listener: {e}")))?;

    println!("http://127.0.0.1:{}", local_addr.port());
    let _ = std::io::stdout().flush();

    let deadline = run_for_secs.map(|s| Instant::now() + Duration::from_secs(s));

    while !stop.load(Ordering::SeqCst) {
        if let Some(d) = deadline {
            if Instant::now() >= d {
                break;
            }
        }
        match listener.accept() {
            Ok((stream, _peer)) => {
                let _ = handle_connection(stream, data_dir);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Ok(())
}

pub fn board_api_payload(data_dir: &Path, view: ViewFilter, now_ms: i64) -> Result<Value> {
    let state = store::load(data_dir)?;
    let filtered = state
        .tickets
        .iter()
        .filter(|t| view.matches(t, now_ms))
        .cloned()
        .collect::<Vec<_>>();
    let cols = ALL_COLUMNS.iter().map(|c| c.as_str()).collect::<Vec<_>>();
    Ok(json!({
        "ok": true,
        "version": state.version,
        "view": view.as_str(),
        "now_ms": now_ms,
        "local_midnight_ms": local_midnight_ms(now_ms),
        "columns": cols,
        "rooms": state.rooms,
        "tickets": filtered,
        "all_tickets": state.tickets,
        "total_session_tickets": state.tickets.len(),
        "session_history": state.session_history,
    }))
}

fn handle_connection(mut stream: TcpStream, data_dir: &Path) -> Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return write_response(
            &mut stream,
            400,
            "application/json",
            br#"{"ok":false,"error":"bad request"}"#,
        );
    }
    let method = parts[0];
    let raw_target = parts[1];

    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse::<usize>().unwrap_or(0).min(64 * 1024);
            }
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let (path, query) = match raw_target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (raw_target, ""),
    };

    match (method, path) {
        ("GET", "/") | ("GET", "/index.html") => write_response(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            PAGE_HTML.as_bytes(),
        ),
        ("GET", "/healthz") => {
            write_response(&mut stream, 200, "application/json", br#"{"ok":true}"#)
        }
        ("GET", "/api/board") => {
            let view = parse_view_query(query);
            let payload = board_api_payload(data_dir, view, now_epoch_ms())?;
            let bytes = serde_json::to_vec(&payload)?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &bytes)
        }
        ("GET", "/api/history") => {
            let state = store::load(data_dir)?;
            let bytes = serde_json::to_vec(&json!({
                "ok": true,
                "count": state.session_history.len(),
                "history": state.session_history,
            }))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &bytes)
        }
        ("POST", "/api/history/clear") => {
            let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            if parsed["confirm"].as_bool() != Some(true) {
                let err =
                    br#"{"ok":false,"error":"explicit confirmation required: {\"confirm\":true}"}"#;
                return write_response(&mut stream, 400, "application/json; charset=utf-8", err);
            }
            let ((cleared_tickets, cleared_events), _) =
                store::mutate(data_dir, |s| Ok(s.clear_session_history()))?;
            let bytes = serde_json::to_vec(&json!({
                "ok": true,
                "cleared_tickets": cleared_tickets,
                "cleared_events": cleared_events,
            }))?;
            write_response(&mut stream, 200, "application/json; charset=utf-8", &bytes)
        }
        _ => write_response(
            &mut stream,
            404,
            "application/json; charset=utf-8",
            br#"{"ok":false,"error":"not found"}"#,
        ),
    }
}

fn parse_view_query(query: &str) -> ViewFilter {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "view" {
                return match v {
                    "day" => ViewFilter::Day,
                    "week" => ViewFilter::Week,
                    _ => ViewFilter::Session,
                };
            }
        }
    }
    ViewFilter::Session
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store, no-cache, must-revalidate, max-age=0\r\nPragma: no-cache\r\nExpires: 0\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}
