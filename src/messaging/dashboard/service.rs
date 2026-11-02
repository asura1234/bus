//! Ownership and discovery for one loopback dashboard per session.
use std::io::{self, BufRead, Read, Write};
use std::net::{SocketAddrV4, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::messaging::storage::io as bus_io;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Endpoint {
    pub(super) url: String,
    pub(super) token: String,
}
impl Endpoint {
    fn address(&self) -> io::Result<SocketAddrV4> {
        let address = self
            .url
            .strip_prefix("http://")
            .and_then(|url| url.strip_suffix('/'))
            .and_then(|address| address.parse::<SocketAddrV4>().ok())
            .filter(|address| {
                address.ip() == &std::net::Ipv4Addr::LOCALHOST && address.port() != 0
            });
        address.ok_or_else(|| io::Error::other("Invalid dashboard loopback endpoint"))
    }
    fn healthy(&self) -> io::Result<bool> {
        let address = self.address()?;
        let timeout = Duration::from_millis(250);
        let mut stream = TcpStream::connect_timeout(&address.into(), timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        write!(
            stream,
            "GET /health HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
        )?;
        let mut bytes = Vec::new();
        stream.take(2048).read_to_end(&mut bytes)?;
        let Some(boundary) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            return Ok(false);
        };
        let body: serde_json::Value =
            serde_json::from_slice(&bytes[boundary + 4..]).map_err(io::Error::other)?;
        Ok(bytes.starts_with(b"HTTP/1.1 200 ") && body["token"].as_str() == Some(&self.token))
    }
}
pub(super) struct Server {
    pub(super) listener: TcpListener,
    pub(super) endpoint: Endpoint,
    _lease: crate::platform::fs::FileLock,
}
pub(super) enum Started {
    Owned(Server),
    Existing(Endpoint),
}

pub(super) fn start(root: &Path) -> io::Result<Started> {
    let runtime = root.join("dashboard-runtime");
    bus_io::private_dir(&runtime)?;
    let endpoint_path = runtime.join("endpoint.json");
    match bus_io::lock(&runtime.join("server.lock")) {
        Ok(lease) => {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
            let address = listener.local_addr()?;
            let endpoint = Endpoint {
                url: format!("http://{address}/"),
                token: format!(
                    "{}-{}-{}",
                    std::process::id(),
                    bus_io::now_ns(),
                    address.port()
                ),
            };
            bus_io::atomic_write(
                &endpoint_path,
                &serde_json::to_vec(&endpoint).map_err(io::Error::other)?,
            )?;
            Ok(Started::Owned(Server {
                listener,
                endpoint,
                _lease: lease,
            }))
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                let discovered = std::fs::read(&endpoint_path)
                    .map_err(|error| format!("read {}: {error}", endpoint_path.display()))
                    .and_then(|bytes| {
                        serde_json::from_slice::<Endpoint>(&bytes)
                            .map_err(|error| format!("JSON {}: {error}", endpoint_path.display()))
                    });
                let problem = match discovered {
                    Ok(endpoint) => match endpoint.healthy() {
                        Ok(true) => return Ok(Started::Existing(endpoint)),
                        Ok(false) => format!(
                            "{}: endpoint identity did not match",
                            endpoint_path.display()
                        ),
                        Err(error) => format!("{}: {error}", endpoint_path.display()),
                    },
                    Err(problem) => problem,
                };
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("Dashboard discovery failed: {problem}"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        Err(error) => Err(error),
    }
}

/// Runs off the UI thread. The child CLI owns/reuses the same per-session service.
pub(crate) fn open_in_browser(root: &Path) -> io::Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--dev", "dashboard"])
        .env("BUS_DATA_DIR", root)
        .env_remove("BUS_SESSION_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Dashboard stdout unavailable"))?;
        let mut url = String::new();
        io::BufReader::new(stdout).take(1024).read_line(&mut url)?;
        let endpoint = Endpoint {
            url: url.trim().into(),
            token: String::new(),
        };
        endpoint.address()?;
        if let Some(mut browser) = crate::platform::open_url(&endpoint.url)? {
            if !browser.wait()?.success() {
                return Err(io::Error::other(
                    "Default browser did not open the dashboard",
                ));
            }
        }
        Ok::<(), io::Error>(())
    })();
    if let Err(error) = result {
        // INFO keeps this diagnostic out of the warning/error notification pipe.
        tracing::info!(event = "bus.dashboard.open_failed", %error);
    }
    // Reap the first service when it eventually exits. Existing-service CLI calls
    // exit immediately; the UI thread never waits on either case.
    let _ = child.wait();
    Ok(())
}

#[cfg(test)]
#[path = "tests/service_test.rs"]
mod tests;
