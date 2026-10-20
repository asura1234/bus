//! Explicit-path frontend for reply integration tests; no ambient socket/env lookup.
use super::*;
use interprocess::local_socket::traits::Listener as _;
use interprocess::local_socket::ListenerNonblockingMode;
use std::thread::JoinHandle;

pub(crate) struct Socket {
    running: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl Socket {
    pub(crate) fn start(path: PathBuf, api_tx: crate::server::api::ApiRequestSender) -> Self {
        let listener = crate::platform::ipc::bind_local_listener(&path).unwrap();
        listener
            .set_nonblocking(ListenerNonblockingMode::Accept)
            .unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let live = running.clone();
        let accept = std::thread::spawn(move || {
            let mut connections = Vec::new();
            while live.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok(stream) => {
                        let tx = api_tx.clone();
                        let live = live.clone();
                        connections.push(std::thread::spawn(move || {
                            // Same decoder, dispatch, timeout and response writer as the server.
                            let _ =
                                super::handle_connection(stream, &tx, &EventHub::default(), &live);
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("accept scratch API: {error}"),
                }
            }
            for connection in connections {
                connection.join().unwrap();
            }
        });
        Self {
            running,
            accept: Some(accept),
            path,
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.accept.take().unwrap().join().unwrap();
        std::fs::remove_file(&self.path).unwrap();
    }
}
