//! The normal native server owns room coordination; viewers own only their projections.
use crate::messaging::coordinator::{BusCommand, BusEvent, BusHandle};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const MAX_EVENTS: usize = 512;
const MAX_CLIENTS: usize = 256;
const CLIENT_TTL: Duration = Duration::from_secs(600);
static RUNNING: AtomicBool = AtomicBool::new(false);
static EVENTS: OnceLock<Mutex<EventLog>> = OnceLock::new();

#[derive(Default)]
struct EventLog {
    sequence: u64,
    events: VecDeque<(u64, BusEvent)>,
    clients: BTreeMap<String, (u64, Instant)>,
}

impl EventLog {
    fn push(&mut self, event: BusEvent) {
        if !matches!(
            event,
            BusEvent::DevFocusRequested { .. }
                | BusEvent::DevQuitRequested
                | BusEvent::DevRestartRequested(_)
                | BusEvent::SettingsChanged(_)
                | BusEvent::FeishuStatusChanged(_)
                | BusEvent::StorageRecovered
        ) {
            return;
        }
        self.sequence = self.sequence.wrapping_add(1);
        self.events.push_back((self.sequence, event));
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }

    fn take(&mut self, client: &str, local: bool, now: Instant) -> Vec<BusEvent> {
        self.clients
            .retain(|_, (_, seen)| now.saturating_duration_since(*seen) < CLIENT_TTL);
        let known = self.clients.get(client).map(|(sequence, _)| *sequence);
        let pid = client
            .split('-')
            .next()
            .and_then(|pid| pid.parse::<u32>().ok());
        let result = self.events.iter().filter(|(sequence, event)| {
            // A new viewer must not replay an old quit/focus request. A restart
            // belongs to one exact still-live TUI, including before its first poll.
            let targeted_restart = matches!(event, BusEvent::DevRestartRequested(plan) if local && pid == Some(plan.tui.pid));
            let fresh = known.is_some_and(|previous| *sequence > previous) || known.is_none() && targeted_restart;
            fresh && match event {
                BusEvent::DevFocusRequested { .. } | BusEvent::DevQuitRequested => local,
                BusEvent::DevRestartRequested(plan) => local && pid == Some(plan.tui.pid),
                _ => true,
            }
        }).map(|(_, event)| event.clone()).collect();
        if self.clients.len() >= MAX_CLIENTS && !self.clients.contains_key(client) {
            if let Some(oldest) = self
                .clients
                .iter()
                .min_by_key(|(_, (_, seen))| *seen)
                .map(|(id, _)| id.clone())
            {
                self.clients.remove(&oldest);
            }
        }
        self.clients.insert(client.to_owned(), (self.sequence, now));
        result
    }
}

pub(crate) fn is_running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

pub(crate) fn client_events(client_id: &str, local_client: bool) -> Vec<BusEvent> {
    EVENTS
        .get()
        .and_then(|events| events.lock().ok())
        .map(|mut events| events.take(client_id, local_client, Instant::now()))
        .unwrap_or_default()
}

pub(crate) struct Service {
    stopped: Arc<AtomicBool>,
    done: mpsc::Receiver<()>,
}

struct Completed(mpsc::Sender<()>);
impl Drop for Completed {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

impl Service {
    /// Starts beside the native event loop; coordinator restoration can call that
    /// loop's API without holding up native startup or spawning another process.
    pub(crate) fn start(root: PathBuf, server_stopped: Arc<AtomicBool>) -> io::Result<Self> {
        let _ = std::fs::remove_file(root.join("coordinator-startup-error"));
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let (done_tx, done) = mpsc::channel();
        std::thread::Builder::new()
            .name("bus-room-service".into())
            .spawn(move || {
                let _completed = Completed(done_tx);
                let target = crate::protocol::api::client::ConnectionTarget::LocalSession(Some(
                    crate::messaging::coordinator::DEFAULT_SESSION.into(),
                ));
                let handle = match BusHandle::start(root.clone(), target) {
                    Ok(handle) => handle,
                    Err(error) => {
                        tracing::error!(%error, "Could not start Bus room coordinator");
                        let _ = std::fs::write(root.join("coordinator-startup-error"), &error);
                        server_stopped.store(true, Ordering::Release);
                        return;
                    }
                };
                // The service owns first-run initialization. Multiple viewers
                // can attach concurrently without each creating the default room.
                if handle
                    .snapshot()
                    .is_some_and(|snapshot| snapshot.state.is_pristine())
                {
                    if let Err(error) =
                        handle.try_send(u64::MAX, BusCommand::CreateRoom("bus".into()))
                    {
                        tracing::warn!(%error, "Could not create the initial Bus room");
                    }
                }
                let events = EVENTS.get_or_init(|| Mutex::new(EventLog::default()));
                if let Ok(mut events) = events.lock() {
                    *events = EventLog::default();
                }
                RUNNING.store(true, Ordering::Release);
                while !stop.load(Ordering::Acquire) && !server_stopped.load(Ordering::Acquire) {
                    for _ in 0..256 {
                        let Some(event) = handle.try_event() else {
                            break;
                        };
                        if let Ok(mut events) = events.lock() {
                            events.push(event);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                // Shutdown belongs to the native server, never to a departing viewer.
                let _ = handle.try_send(0, BusCommand::Shutdown);
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline {
                    if matches!(
                        handle.try_event(),
                        Some(BusEvent::CommandFinished { command_id: 0, .. })
                    ) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                RUNNING.store(false, Ordering::Release);
            })?;
        Ok(Self { stopped, done })
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        // Let the owner fold its durable journal before this server process exits.
        // A failing native operation cannot hold shutdown open indefinitely.
        if self.done.recv_timeout(Duration::from_secs(3)).is_err() {
            tracing::warn!("Room coordinator did not acknowledge shutdown before deadline");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_machine_service_events_are_client_scoped_and_remote_cannot_quit() {
        let mut log = EventLog::default();
        let now = Instant::now();
        assert!(log.take("10-a", true, now).is_empty());
        assert!(log.take("20-a", false, now).is_empty());
        log.push(BusEvent::DevQuitRequested);
        assert!(matches!(
            log.take("10-a", true, now).as_slice(),
            [BusEvent::DevQuitRequested]
        ));
        assert!(log.take("10-a", true, now).is_empty());
        assert!(log.take("20-a", false, now).is_empty());
        assert!(
            log.take("30-new", true, now).is_empty(),
            "new viewers cannot replay old quit"
        );
    }

    #[test]
    fn multi_machine_service_event_log_stays_bounded_without_viewers() {
        let mut log = EventLog::default();
        for _ in 0..MAX_EVENTS * 4 {
            log.push(BusEvent::StorageRecovered);
        }
        assert_eq!(log.events.len(), MAX_EVENTS);
        for i in 0..MAX_CLIENTS * 2 {
            log.take(&i.to_string(), false, Instant::now());
        }
        assert_eq!(log.clients.len(), MAX_CLIENTS);
    }
    #[test]
    fn multi_machine_restart_event_reaches_only_its_original_local_tui() {
        let mut log = EventLog::default();
        let plan = crate::utils::restart_record::HandoffPlan {
            id: "restart".into(),
            root: "/private/test".into(),
            old_binary: "/old".into(),
            new_binary: "/new".into(),
            old_sha: "a".repeat(40),
            new_sha: "b".repeat(40),
            who: "test".into(),
            tui: crate::utils::restart_record::LivePid { pid: 42, birth: 1 },
            session_id: None,
            explicit_data_dir: true,
            dev: true,
            yolo: false,
            clear_data: false,
        };
        log.push(BusEvent::DevRestartRequested(plan.clone()));
        let now = Instant::now();
        assert!(log.take("41-a", true, now).is_empty());
        assert!(log.take("42-remote", false, now).is_empty());
        assert!(
            matches!(log.take("42-local", true, now).as_slice(), [BusEvent::DevRestartRequested(received)] if received == &plan)
        );
        assert!(log.take("42-local", true, now).is_empty());
    }
}
