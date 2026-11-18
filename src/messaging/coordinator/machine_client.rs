//! Non-owning clients of a running coordinator. Reconnection reads state only;
//! a failed mutation is reported once and is never retried or spooled.
use super::{BusCommand, BusEvent, BusHandle, BusSnapshot, BusState};
use crate::messaging::control::{Request, Response};
use crate::messaging::machine_routes;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

enum Endpoint {
    Local(PathBuf),
    Remote {
        target: String,
        connection: Option<crate::messaging::machines::ControlConnection>,
    },
}

impl Endpoint {
    fn request(&mut self, request: &Request) -> Result<Response, String> {
        match self {
            Self::Local(root) => crate::messaging::control::request(root, request),
            Self::Remote { target, connection } => {
                if connection.is_none() {
                    *connection = Some(crate::messaging::machines::ControlConnection::connect(
                        target,
                    )?);
                }
                let result = connection.as_mut().expect("connected").request(request);
                if result.is_err() {
                    *connection = None;
                }
                result
            }
        }
    }
}

impl BusHandle {
    pub(crate) fn is_connected(&self) -> bool {
        self.connected
            .as_ref()
            .is_none_or(|connected| connected.load(Ordering::Acquire))
    }

    /// Returns immediately, including when the machine is currently down.
    pub(crate) fn connect_remote_named(label: String, target: String) -> Result<Self, String> {
        let cache = cache_path(&target);
        Self::connect_proxy(
            Endpoint::Remote {
                target,
                connection: None,
            },
            cache,
            label,
        )
    }

    pub(crate) fn connect_existing(data_dir: PathBuf) -> Result<Self, String> {
        let response = crate::messaging::control::request_with_timeout(
            &data_dir,
            &Request {
                id: format!("attach-{}-{}", std::process::id(), super::io::now_ns()),
                method: "client.snapshot".into(),
                params: json!({"client_id":"local-attach"}),
            },
            Duration::from_secs(3),
        )?;
        let (mut snapshot, _, _, _) = decode(response)?;
        snapshot.last_command_id = 0;
        let handle = Self::connect_proxy(
            Endpoint::Local(data_dir),
            None,
            crate::platform::hostname().unwrap_or_else(|| "this-machine".into()),
        )?;
        if let Ok(mut shared) = handle.snapshots.lock() {
            *shared = Arc::new(snapshot);
        }
        if let Some(connected) = &handle.connected {
            connected.store(true, Ordering::Release);
        }
        Ok(handle)
    }

    fn connect_proxy(
        mut endpoint: Endpoint,
        cache: Option<PathBuf>,
        label: String,
    ) -> Result<Self, String> {
        let local_client = matches!(&endpoint, Endpoint::Local(_));
        let initial = cached_snapshot(cache.as_ref());
        let snapshots = Arc::new(Mutex::new(Arc::new(initial)));
        let connected = Arc::new(AtomicBool::new(false));
        let (commands, receiver) = mpsc::sync_channel(super::COMMAND_QUEUE_CAPACITY);
        let (event_tx, events) = mpsc::channel();
        let direct_events = event_tx.clone();
        let shared = Arc::clone(&snapshots);
        let live = Arc::clone(&connected);
        let (route_lease, route_jobs) = machine_routes::register(label.clone(), Arc::clone(&live));
        let (completion_tx, completion_rx) = mpsc::channel::<machine_routes::Completion>();
        static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);
        let client = format!(
            "{}-{}-{}",
            std::process::id(),
            super::io::now_ns(),
            NEXT_CLIENT.fetch_add(1, Ordering::Relaxed)
        );
        std::thread::Builder::new()
            .name("machine-client".into())
            .spawn(move || {
                let _route_lease = route_lease;
                let mut completions = Vec::new();
                let mut sequence = 0_u64;
                let mut next_poll = std::time::Instant::now();
                loop {
                    // Transit jobs are asynchronous across links. Never wait for
                    // a second proxy while holding this connection's poll loop.
                    while let Ok(forward) = route_jobs.try_recv() {
                        let response = if !live.load(Ordering::Acquire) {
                            Response::failure(&forward.job.request.id, "machine_unavailable", "Destination link is down; nothing was sent or queued")
                        } else {
                            match endpoint.request(&forward.job.request) {
                                Ok(response) => response,
                                Err(error) => {
                                    live.store(false, Ordering::Release);
                                    machine_routes::unknown(&forward.job.request.id, &format!("Destination acknowledgement was lost: {error}; request was not retried"))
                                }
                            }
                        };
                        let _ = forward.complete.send(machine_routes::Completion { id:forward.job.id, response });
                    }
                    completions.extend(completion_rx.try_iter());
                    if !completions.is_empty() { next_poll = std::time::Instant::now(); }
                    let wait = next_poll.saturating_duration_since(std::time::Instant::now()).min(Duration::from_millis(40));
                    let command = match receiver.recv_timeout(wait) {
                        Ok(command) => Some(command),
                        Err(mpsc::RecvTimeoutError::Timeout) if std::time::Instant::now() < next_poll => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    let command_id = command.as_ref().map(|(id, _)| *id);
                    if let Some((id, BusCommand::Shutdown)) = command.as_ref() {
                        finish_local(&shared, &event_tx, *id, Ok(()));
                        break;
                    }
                    if let Some((id, _)) =
                        command.as_ref().filter(|_| !live.load(Ordering::Acquire))
                    {
                        finish_local(
                            &shared,
                            &event_tx,
                            *id,
                            Err("Machine is unavailable; command was not sent".into()),
                        );
                        continue;
                    }
                    sequence = sequence.wrapping_add(1);
                    let mut request = Request {
                        id: format!("{client}-{sequence}"),
                        method: if command.is_some() {
                            "client.command"
                        } else {
                            "client.snapshot"
                        }
                        .into(),
                        params: match command {
                            Some((id, command)) => {
                                json!({"client_id":client,"command_id":id,"command":command})
                            }
                            None => {
                                let mut params = machine_routes::exchange_params();
                                params["client_id"] = json!(client);
                                params["local_client"] = json!(local_client);
                                params["completions"] = json!(std::mem::take(&mut completions));
                                params
                            },
                        },
                    };
                    request.params["local_client"] = json!(local_client);
                    #[cfg(feature = "dev-tools")]
                    if local_client {
                        request.params["ui_state"] = super::ui_hits::client_state();
                    }
                    let response = endpoint.request(&request);
                    #[cfg(feature = "dev-tools")]
                    if local_client {
                        if let Ok(response) = &response {
                            if let Some(holding) = response.result["ui_ack_hold"].as_bool() {
                                super::ui_hits::receive_ack_hold(holding);
                            }
                        }
                    }
                    match response.and_then(decode) {
                        Ok((mut snapshot, events, jobs, identity)) => {
                            machine_routes::identify_route(&label, &identity);
                            for job in jobs { machine_routes::dispatch(job, &completion_tx); }
                            live.store(true, Ordering::Release);
                            if let Ok(mut current) = shared.lock() {
                                snapshot.last_command_id =
                                    current.last_command_id.max(command_id.unwrap_or(0));
                                // Reconnection and local failures must always replace the previous projection.
                                snapshot.revision =
                                    snapshot.revision.max(current.revision.saturating_add(1));
                                *current = Arc::new(snapshot.clone());
                            }
                            for event in events {
                                let _ = event_tx.send(event);
                            }
                            if let Some(path) = &cache {
                                save_cache(path, &snapshot);
                            }
                            next_poll = std::time::Instant::now() + Duration::from_millis(300);
                        }
                        Err(error) => {
                            live.store(false, Ordering::Release);
                            if let Some(id) = command_id {
                                finish_local(&shared, &event_tx, id, Err(error.clone()));
                            }
                            // Any commands admitted before failure are failed now. They
                            // must not silently become an outbox for a later connection.
                            while let Ok((id, _)) = receiver.try_recv() {
                                finish_local(
                                    &shared,
                                    &event_tx,
                                    id,
                                    Err("Machine disconnected; command was not sent".into()),
                                );
                            }
                            tracing::debug!(%error, "machine client unavailable");
                            next_poll = std::time::Instant::now() + Duration::from_secs(2);
                        }
                    }
                }
                live.store(false, Ordering::Release);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            _control: None,
            connected: Some(connected),
            proxy_events: Some(direct_events),
            commands,
            snapshots,
            events,
        })
    }
}

fn finish_local(
    shared: &Mutex<Arc<BusSnapshot>>,
    events: &mpsc::Sender<BusEvent>,
    id: u64,
    result: Result<(), String>,
) {
    if let Ok(mut snapshot) = shared.lock() {
        let updated = Arc::make_mut(&mut snapshot);
        updated.last_command_id = updated.last_command_id.max(id);
        updated.revision = updated.revision.saturating_add(1);
    }
    let _ = events.send(BusEvent::CommandFinished {
        command_id: id,
        result,
    });
}

pub(super) fn finish_disconnected(
    shared: &Mutex<Arc<BusSnapshot>>,
    events: &mpsc::Sender<BusEvent>,
    id: u64,
) {
    finish_local(
        shared,
        events,
        id,
        Err("Machine is unavailable; command was not sent".into()),
    );
}

fn decode(
    response: Response,
) -> Result<(BusSnapshot, Vec<BusEvent>, Vec<machine_routes::Job>, String), String> {
    if !response.ok {
        return Err(response
            .error
            .map_or_else(|| "Remote command failed".into(), |error| error.message));
    }
    let mut snapshot: BusSnapshot =
        serde_json::from_value(response.result["snapshot"].clone()).map_err(|e| e.to_string())?;
    let history =
        serde_json::from_value(response.result["history"].clone()).map_err(|e| e.to_string())?;
    snapshot.state.install_client_history(history);
    let events =
        serde_json::from_value(response.result["events"].clone()).map_err(|e| e.to_string())?;
    let jobs = response
        .result
        .get("forward_jobs")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let identity = response.result["machine_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    Ok((snapshot, events, jobs, identity))
}

fn cache_path(target: &str) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    crate::utils::env::bus_data_dir().map(|root| {
        root.join("machine-cache")
            .join(format!("{:x}.json", Sha256::digest(target.as_bytes())))
    })
}

fn save_cache(path: &std::path::Path, snapshot: &BusSnapshot) {
    let cached = BusSnapshot {
        state: snapshot.state.machine_room_cache(),
        revision: 0,
        last_command_id: 0,
        error: None,
    };
    if let Some(parent) = path.parent() {
        if super::io::private_dir(parent).is_err() {
            return;
        }
    }
    if let Ok(bytes) = serde_json::to_vec(&cached) {
        if std::fs::read(path).is_ok_and(|previous| previous == bytes) {
            return;
        }
        let _ = crate::messaging::storage::io::atomic_write(path, &bytes);
    }
}

fn cached_snapshot(cache: Option<&PathBuf>) -> BusSnapshot {
    cache
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<BusSnapshot>(&bytes).ok())
        .unwrap_or(BusSnapshot {
            state: BusState::default(),
            revision: 0,
            last_command_id: 0,
            error: None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_cache_contains_rooms_without_agents_or_drafts() {
        let mut state = BusState::default();
        let room = state.create_room("same-name").unwrap();
        state.set_draft_text(room, "private draft").unwrap();
        state
            .create_agent(
                room,
                "worker",
                super::super::Provider::Codex,
                "/remote".into(),
                None,
            )
            .unwrap();
        let cached = state.machine_room_cache();
        assert_eq!(cached.room(room).unwrap().name, "same-name");
        assert_eq!(cached.room(room).unwrap().draft.text, "");
        assert_eq!(cached.agents().count(), 0);
    }

    #[test]
    fn disconnected_machine_command_fails_once_without_entering_outbox() {
        let (commands, receiver) = mpsc::sync_channel(4);
        let (tx, events) = mpsc::channel();
        let snapshot = Arc::new(Mutex::new(Arc::new(BusSnapshot {
            state: BusState::default(),
            revision: 0,
            last_command_id: 0,
            error: None,
        })));
        let handle = BusHandle {
            _control: None,
            connected: Some(Arc::new(AtomicBool::new(false))),
            proxy_events: Some(tx),
            commands,
            snapshots: snapshot,
            events,
        };
        handle
            .try_send(17, BusCommand::CreateRoom("offline".into()))
            .unwrap();
        assert!(receiver.try_recv().is_err());
        assert!(matches!(
            handle.try_event(),
            Some(BusEvent::CommandFinished {
                command_id: 17,
                result: Err(_)
            })
        ));
        assert!(handle.try_event().is_none());
        assert_eq!(handle.snapshot().unwrap().last_command_id, 17);
        handle
            .connected
            .as_ref()
            .unwrap()
            .store(true, Ordering::Release);
        assert!(receiver.try_recv().is_err());
    }
}
