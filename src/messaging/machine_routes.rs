//! Ephemeral routes carried by an already-connected viewing Bus. This is an
//! in-process exchange, never an outbox, service, or persistent retry queue.
use crate::messaging::control::{Request, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

const HEARTBEAT: Duration = Duration::from_secs(4);
const FORWARD_DEADLINE: Duration = Duration::from_secs(90);
// Leave interactive control slots available for the viewer heartbeat.
const MAX_PENDING: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Job {
    pub(crate) id: String,
    pub(crate) machine: String,
    pub(crate) identity: String,
    pub(crate) request: Request,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Completion {
    pub(crate) id: String,
    pub(crate) response: Response,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Advertisement {
    pub(crate) label: String,
    pub(crate) identity: String,
}

struct Viewer {
    routes: BTreeMap<String, String>,
    seen: Instant,
}

struct Pending {
    viewer: String,
    request_id: String,
    reply: mpsc::SyncSender<Response>,
    job: Option<Job>,
    deadline: Instant,
    dispatched: bool,
}

#[derive(Default)]
struct Broker {
    viewers: BTreeMap<String, Viewer>,
    pending: BTreeMap<String, Pending>,
    spent: BTreeMap<String, Instant>,
}

fn broker() -> &'static Mutex<Broker> {
    static BROKER: OnceLock<Mutex<Broker>> = OnceLock::new();
    BROKER.get_or_init(Default::default)
}

/// Runs on control socket admission, outside the coordinator's single writer.
pub(crate) fn forward(request: &Request) -> Result<mpsc::Receiver<Response>, Response> {
    let machine = request.params["machine"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Response::failure(
                &request.id,
                "invalid_request",
                "Missing destination machine",
            )
        })?;
    let inner: Request =
        serde_json::from_value(request.params["request"].clone()).map_err(|_| {
            Response::failure(&request.id, "invalid_request", "Invalid forwarded request")
        })?;
    if inner.method == "machine.forward"
        || inner.method.starts_with("client.")
        || inner.id.is_empty()
    {
        return Err(Response::failure(
            &request.id,
            "invalid_request",
            "A route cannot forward routing or client-control requests",
        ));
    }
    let mut broker = broker().lock().map_err(|_| {
        Response::failure(
            &request.id,
            "machine_unavailable",
            "Route registry unavailable",
        )
    })?;
    broker.expire();
    let matches = broker
        .viewers
        .iter()
        .filter_map(|(viewer, entry)| entry.routes.get(machine).map(|identity| (viewer, identity)))
        .collect::<Vec<_>>();
    if matches
        .iter()
        .map(|(_, identity)| *identity)
        .collect::<BTreeSet<_>>()
        .len()
        > 1
    {
        return Err(Response::failure(
            &request.id,
            "ambiguous_machine",
            "Connected viewers use this label for different machines; choose a unique label",
        ));
    }
    let (viewer, destination_identity) = matches
        .first()
        .map(|(viewer, identity)| ((*viewer).clone(), (*identity).clone()))
        .ok_or_else(|| {
            Response::failure(
                &request.id,
                "machine_unavailable",
                "No connected viewer has a live route to this machine; nothing was queued",
            )
        })?;
    let identity = format!("{}:{}", machine, inner.id);
    if broker.spent.contains_key(&identity) {
        return Err(Response::failure(
            &request.id,
            "forward_already_attempted",
            "This forward was already attempted; its outcome may be unknown. It was not sent again",
        ));
    }
    if broker.pending.len() >= MAX_PENDING || broker.spent.len() >= 4096 {
        return Err(Response::failure(
            &request.id,
            "route_busy",
            "Live route capacity reached; nothing was queued",
        ));
    }
    static NEXT_JOB: AtomicU64 = AtomicU64::new(1);
    let id = format!(
        "{}-{}-{}",
        std::process::id(),
        crate::utils::time::now_ns(),
        NEXT_JOB.fetch_add(1, Ordering::Relaxed)
    );
    let (tx, rx) = mpsc::sync_channel(1);
    broker.spent.insert(identity, Instant::now());
    broker.pending.insert(
        id.clone(),
        Pending {
            viewer,
            request_id: request.id.clone(),
            reply: tx,
            job: Some(Job {
                id,
                machine: machine.into(),
                identity: destination_identity,
                request: inner,
            }),
            deadline: Instant::now() + HEARTBEAT,
            dispatched: false,
        },
    );
    Ok(rx)
}

impl Broker {
    fn expire(&mut self) {
        let now = Instant::now();
        self.viewers
            .retain(|_, viewer| now.duration_since(viewer.seen) < HEARTBEAT);
        self.spent
            .retain(|_, at| now.duration_since(*at) < Duration::from_secs(600));
        let expired = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                now >= pending.deadline || !self.viewers.contains_key(&pending.viewer)
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(pending) = self.pending.remove(&id) {
                let response = if pending.dispatched {
                    unknown(&pending.request_id, "Viewer disconnected before acknowledgement; destination acceptance is unknown")
                } else {
                    Response::failure(
                        &pending.request_id,
                        "machine_unavailable",
                        "Route disappeared before dispatch; nothing was queued",
                    )
                };
                let _ = pending.reply.try_send(response);
            }
        }
    }
}

pub(crate) fn expire() {
    if let Ok(mut broker) = broker().lock() {
        broker.expire();
    }
}

/// Each viewer is identified by its process lifetime, across all of its links.
/// Snapshot polling is the sole heartbeat; stale viewer routes cannot admit work.
pub(crate) fn exchange(
    viewer_id: &str,
    routes: Vec<Advertisement>,
    completions: Vec<Completion>,
) -> Vec<Job> {
    if viewer_id.is_empty() || viewer_id.len() > 128 {
        return Vec::new();
    }
    let Ok(mut broker) = broker().lock() else {
        return Vec::new();
    };
    broker.expire();
    broker.viewers.insert(
        viewer_id.into(),
        Viewer {
            routes: routes
                .into_iter()
                .filter(|route| {
                    !route.identity.is_empty()
                        && route.identity.len() <= 128
                        && !route.label.is_empty()
                        && route.label.len() <= 255
                })
                .take(64)
                .map(|route| (route.label, route.identity))
                .collect(),
            seen: Instant::now(),
        },
    );
    for completion in completions.into_iter().take(MAX_PENDING) {
        if broker
            .pending
            .get(&completion.id)
            .is_none_or(|pending| pending.viewer != viewer_id)
        {
            continue;
        }
        if let Some(pending) = broker.pending.remove(&completion.id) {
            let _ = pending.reply.try_send(Response::success(
                &pending.request_id,
                json!(completion.response),
            ));
        }
    }
    broker
        .pending
        .values_mut()
        .filter(|pending| pending.viewer == viewer_id)
        .filter_map(|pending| {
            let job = pending.job.take()?;
            pending.dispatched = true;
            pending.deadline = Instant::now() + FORWARD_DEADLINE;
            Some(job)
        })
        .take(32)
        .collect()
}

pub(crate) fn unknown(id: &str, message: &str) -> Response {
    let mut response = Response::failure(id, "machine_outcome_unknown", message);
    response.result = json!({"applied":"unknown"});
    response
}

pub(crate) struct LocalJob {
    pub(crate) job: Job,
    pub(crate) complete: mpsc::Sender<Completion>,
}

struct Route {
    identity: String,
    live: Arc<AtomicBool>,
    jobs: mpsc::Sender<LocalJob>,
    generation: u64,
}

fn routes() -> &'static Mutex<BTreeMap<String, Route>> {
    static ROUTES: OnceLock<Mutex<BTreeMap<String, Route>>> = OnceLock::new();
    ROUTES.get_or_init(Default::default)
}

pub(crate) struct RouteLease {
    label: String,
    generation: u64,
}
impl Drop for RouteLease {
    fn drop(&mut self) {
        if let Ok(mut routes) = routes().lock() {
            if routes
                .get(&self.label)
                .is_some_and(|route| route.generation == self.generation)
            {
                routes.remove(&self.label);
            }
        }
    }
}

pub(crate) fn register(
    label: String,
    live: Arc<AtomicBool>,
) -> (RouteLease, mpsc::Receiver<LocalJob>) {
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let (jobs, receiver) = mpsc::channel();
    if let Ok(mut routes) = routes().lock() {
        routes.insert(
            label.clone(),
            Route {
                identity: String::new(),
                live,
                jobs,
                generation,
            },
        );
    }
    (RouteLease { label, generation }, receiver)
}

pub(crate) fn identify_route(label: &str, identity: &str) {
    if let Ok(mut routes) = routes().lock() {
        if let Some(route) = routes.get_mut(label) {
            route.identity = identity.into();
        }
    }
}

pub(crate) fn connected_routes() -> Vec<Advertisement> {
    routes()
        .lock()
        .map(|routes| {
            routes
                .iter()
                .filter(|(_, route)| {
                    route.live.load(Ordering::Acquire) && !route.identity.is_empty()
                })
                .map(|(label, route)| Advertisement {
                    label: label.clone(),
                    identity: route.identity.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn dispatch(job: Job, completion: &mpsc::Sender<Completion>) {
    let sent = routes()
        .lock()
        .ok()
        .and_then(|routes| {
            routes
                .get(&job.machine)
                .filter(|route| {
                    route.live.load(Ordering::Acquire) && route.identity == job.identity
                })
                .map(|route| {
                    route
                        .jobs
                        .send(LocalJob {
                            job: job.clone(),
                            complete: completion.clone(),
                        })
                        .is_ok()
                })
        })
        .unwrap_or(false);
    if !sent {
        let _ = completion.send(Completion {
            id: job.id,
            response: Response::failure(
                &job.request.id,
                "machine_unavailable",
                "Destination link is down or changed; message was not sent or queued",
            ),
        });
    }
}

pub(crate) fn viewer_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        format!(
            "viewer-{}-{}",
            std::process::id(),
            crate::utils::time::now_ns()
        )
    })
}

/// A durable opaque identity independent of the user's hostname or label.
/// The coordinator's ownership lock serializes its first creation.
pub(crate) fn machine_identity(root: &std::path::Path) -> Result<String, String> {
    let path = root.join("machine-id");
    match std::fs::read_to_string(&path) {
        Ok(id) if id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            return Ok(id)
        }
        Ok(_) => return Err("Invalid Bus machine identity".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    use std::hash::{BuildHasher, Hasher};
    let random = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(crate::utils::time::now_ns());
        hasher.write_u32(std::process::id());
        hasher.finish()
    };
    let id = format!("{:016x}{:016x}", random(), random());
    crate::messaging::storage::io::atomic_write(&path, id.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(id)
}

pub(crate) fn exchange_params() -> Value {
    json!({"viewer_id":viewer_id(),"routes":connected_routes()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connected_viewer_forwards_once_and_lost_ack_is_never_replayed() {
        let viewer = format!("test-{}", crate::utils::time::now_ns());
        let machine = format!("C-{}", viewer);
        exchange(
            &viewer,
            vec![Advertisement {
                label: machine.clone(),
                identity: "C-identity".into(),
            }],
            vec![],
        );
        let request = Request {
            id: format!("outer-{viewer}"),
            method: "machine.forward".into(),
            params: json!({"machine":machine,"request":{"id":format!("inner-{viewer}"),"method":"message.send","params":{"text":"one"}}}),
        };
        let reply = forward(&request).unwrap();
        let jobs = exchange(
            &viewer,
            vec![Advertisement {
                label: machine.clone(),
                identity: "C-identity".into(),
            }],
            vec![],
        );
        assert_eq!(jobs.len(), 1);
        assert!(exchange(
            &viewer,
            vec![Advertisement {
                label: machine.clone(),
                identity: "C-identity".into()
            }],
            vec![]
        )
        .is_empty());
        assert!(forward(&request).is_err());
        let result = Response::success(&jobs[0].request.id, json!({"message_id":1}));
        exchange(
            &viewer,
            vec![Advertisement {
                label: machine,
                identity: "C-identity".into(),
            }],
            vec![Completion {
                id: jobs[0].id.clone(),
                response: result,
            }],
        );
        assert_eq!(
            reply.recv_timeout(Duration::from_secs(1)).unwrap().result["result"]["message_id"],
            1
        );
    }
    #[test]
    fn absent_viewer_route_does_not_admit_or_queue() {
        let request = Request {
            id: "no-route".into(),
            method: "machine.forward".into(),
            params: json!({"machine":"never-configured-route","request":{"id":"inner-no-route","method":"message.send","params":{}}}),
        };
        let error = forward(&request).unwrap_err();
        assert_eq!(error.error.unwrap().code, "machine_unavailable");
    }

    #[test]
    fn lost_ack_after_dispatch_reports_unknown_and_never_replays() {
        let viewer = format!("lost-{}", crate::utils::time::now_ns());
        let label = format!("target-{viewer}");
        let advert = Advertisement {
            label: label.clone(),
            identity: viewer.clone(),
        };
        exchange(&viewer, vec![advert.clone()], vec![]);
        let request = Request {
            id: format!("outer-{viewer}"),
            method: "machine.forward".into(),
            params: json!({"machine":label,"request":{"id":format!("inner-{viewer}"),"method":"message.send","params":{}}}),
        };
        let receipt = forward(&request).unwrap();
        let jobs = exchange(&viewer, vec![advert.clone()], vec![]);
        assert_eq!(jobs.len(), 1);
        // Destination may have accepted; only its acknowledgement is lost.
        broker()
            .lock()
            .unwrap()
            .pending
            .get_mut(&jobs[0].id)
            .unwrap()
            .deadline = Instant::now() - Duration::from_secs(1);
        expire();
        let unknown = receipt.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(unknown.result["applied"], "unknown");
        assert!(exchange(&viewer, vec![advert], vec![]).is_empty());
        assert!(forward(&request).is_err());
    }

    #[test]
    fn stale_advertisement_cannot_send_through_a_down_destination() {
        let label = format!("down-{}", crate::utils::time::now_ns());
        let (_lease, jobs) = register(label.clone(), Arc::new(AtomicBool::new(false)));
        let (tx, rx) = mpsc::channel();
        dispatch(
            Job {
                id: "down-job".into(),
                machine: label,
                identity: "down-identity".into(),
                request: Request {
                    id: "down-request".into(),
                    method: "message.send".into(),
                    params: json!({}),
                },
            },
            &tx,
        );
        assert!(jobs.try_recv().is_err());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1))
                .unwrap()
                .response
                .error
                .unwrap()
                .code,
            "machine_unavailable"
        );
    }

    #[test]
    fn machine_target_replacement_cannot_redirect_a_pending_send() {
        let label = format!("replaced-{}", crate::utils::time::now_ns());
        let (_lease, jobs) = register(label.clone(), Arc::new(AtomicBool::new(true)));
        identify_route(&label, "replacement-bus");
        let (tx, rx) = mpsc::channel();
        dispatch(
            Job {
                id: "replacement-job".into(),
                machine: label,
                identity: "original-bus".into(),
                request: Request {
                    id: "replacement-request".into(),
                    method: "message.send".into(),
                    params: json!({}),
                },
            },
            &tx,
        );
        assert!(jobs.try_recv().is_err());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1))
                .unwrap()
                .response
                .error
                .unwrap()
                .code,
            "machine_unavailable"
        );
    }
}
