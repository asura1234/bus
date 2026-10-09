use std::io;
use std::time::{Duration, Instant};

use crate::protocol::wire::ClientMessage;

#[path = "writer.rs"]
mod writer;

pub(crate) use writer::NativeEndpointTransport;

const EXIT_FLUSH_GRACE: Duration = Duration::from_millis(250);

pub(crate) trait EndpointTransport: Send {
    fn send(&mut self, message: &ClientMessage) -> io::Result<()>;

    fn disconnect(&mut self) {}

    fn flush(&mut self, _deadline: Instant) -> io::Result<()> {
        Ok(())
    }

    fn take_error(&mut self) -> Option<io::Error> {
        None
    }
}

/// The client's one connection to its local server. A failed send or a lost reader drops the
/// transport and keeps the first error for the event loop, which then ends the client.
pub(crate) struct ServerConnection {
    transport: Option<Box<dyn EndpointTransport>>,
    failure: Option<io::Error>,
}

impl ServerConnection {
    pub(crate) fn new(transport: impl EndpointTransport + 'static) -> Self {
        Self {
            transport: Some(Box::new(transport)),
            failure: None,
        }
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.transport.is_some()
    }

    pub(crate) fn send(&mut self, message: &ClientMessage) -> io::Result<()> {
        let result = match self.transport.as_mut() {
            Some(transport) => transport.send(message),
            None => Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "server connection is closed",
            )),
        };
        if let Err(error) = &result {
            self.fail(io::Error::new(error.kind(), error.to_string()));
        }
        result
    }

    pub(crate) fn fail(&mut self, error: io::Error) {
        if let Some(mut transport) = self.transport.take() {
            transport.disconnect();
            self.failure = Some(error);
        }
    }

    pub(crate) fn take_failure(&mut self) -> Option<io::Error> {
        if let Some(error) = self
            .transport
            .as_mut()
            .and_then(|transport| transport.take_error())
        {
            self.fail(error);
        }
        self.failure.take()
    }
}

impl Drop for ServerConnection {
    fn drop(&mut self) {
        let Some(mut transport) = self.transport.take() else {
            return;
        };
        let _ = transport.send(&ClientMessage::Detach);
        let _ = transport.flush(Instant::now() + EXIT_FLUSH_GRACE);
        transport.disconnect();
    }
}

pub(crate) enum EndpointControlMessage {
    Snapshot(Box<crate::protocol::wire::ClientShellSnapshot>),
    Ignored,
}

pub(crate) fn decode_endpoint_control(
    kind: &str,
    data: &str,
) -> Result<EndpointControlMessage, String> {
    if kind == crate::protocol::wire::handshake::ENDPOINT_SNAPSHOT_KIND {
        let snapshot = serde_json::from_str(data)
            .map_err(|error| format!("invalid endpoint snapshot: {error}"))?;
        return Ok(EndpointControlMessage::Snapshot(Box::new(snapshot)));
    }
    if kind.starts_with("shell.snapshot.") {
        return Err(format!(
            "unsupported mandatory endpoint snapshot codec {kind:?}"
        ));
    }
    Ok(EndpointControlMessage::Ignored)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct FakeTransport {
        sent: Arc<Mutex<Vec<ClientMessage>>>,
        fail: bool,
    }

    impl EndpointTransport for FakeTransport {
        fn send(&mut self, message: &ClientMessage) -> io::Result<()> {
            if self.fail {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "fake failure"));
            }
            self.sent.lock().unwrap().push(message.clone());
            Ok(())
        }
    }

    #[test]
    fn a_failed_send_closes_the_connection_and_reports_once() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let mut connection = ServerConnection::new(FakeTransport {
            sent: sent.clone(),
            fail: true,
        });
        assert!(connection.send(&ClientMessage::Detach).is_err());
        assert!(!connection.is_connected());
        assert_eq!(
            connection.take_failure().map(|error| error.kind()),
            Some(io::ErrorKind::BrokenPipe)
        );
        assert!(connection.take_failure().is_none());
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn dropping_the_connection_sends_detach() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        drop(ServerConnection::new(FakeTransport {
            sent: sent.clone(),
            fail: false,
        }));
        assert_eq!(*sent.lock().unwrap(), vec![ClientMessage::Detach]);
    }

    #[test]
    fn unknown_optional_controls_are_ignored() {
        assert!(matches!(
            decode_endpoint_control("future.optional", "not json").unwrap(),
            EndpointControlMessage::Ignored
        ));
    }

    #[test]
    fn unknown_snapshot_codecs_are_rejected() {
        assert_eq!(
            decode_endpoint_control("shell.snapshot.v2", "{}")
                .err()
                .as_deref(),
            Some("unsupported mandatory endpoint snapshot codec \"shell.snapshot.v2\"")
        );
    }
}
