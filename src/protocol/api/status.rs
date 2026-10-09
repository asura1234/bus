use std::io;
use std::path::Path;
use std::time::Duration;

use crate::protocol::api::schema::{Method, Request, ResponseResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub version: String,
    pub protocol: u32,
    pub capabilities: Option<crate::protocol::api::schema::ServerCapabilities>,
}

pub fn read_runtime_status_at(
    socket_path: &Path,
    timeout: Duration,
) -> io::Result<Option<RuntimeStatus>> {
    if !socket_path.exists() {
        return Ok(None);
    }

    let client = crate::protocol::api::client::ApiClient::for_target(
        crate::protocol::api::client::ConnectionTarget::SocketPath(socket_path.to_path_buf()),
    );
    let request = Request {
        id: "runtime:status".into(),
        method: Method::Ping(crate::protocol::api::schema::PingParams::default()),
    };
    let response = client
        .request_value_with_timeout(&request, timeout)
        .and_then(crate::protocol::api::client::parse_response_value);
    let response = match response {
        Ok(response) => response,
        Err(crate::protocol::api::client::ApiClientError::Io(err))
            if matches!(
                err.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::NotFound
                    | io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(None);
        }
        Err(err) => return Err(io::Error::other(err)),
    };
    match response.result {
        ResponseResult::Pong {
            version,
            protocol,
            capabilities,
        } => Ok(Some(RuntimeStatus {
            version,
            protocol,
            capabilities,
        })),
        result => Err(io::Error::other(format!(
            "server status request returned unexpected result: {result:?}"
        ))),
    }
}
