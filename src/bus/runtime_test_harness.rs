//! Test-only Worker harness for consumers outside the runtime: every fact comes from real
//! Worker command and operation handlers, never from a hand-written `BusState`.
use super::*;
use crate::bus::{
    orchestrator::{
        CancellationToken, Capability, ModelAdapter, ModelRequest, ModelResponse, ParticipantId,
        ProviderError, RoomOperation,
    },
    transport::TransportError,
};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HARNESS_ID: AtomicU64 = AtomicU64::new(1);

/// The harness settles only explicit commands; the model loop never proposes work.
struct DormantModel;

impl ModelAdapter for DormantModel {
    async fn complete(
        &self,
        _request: ModelRequest,
        _credential: crate::bus::credentials::CredentialGeneration,
        _cancel: CancellationToken,
    ) -> Result<ModelResponse, ProviderError> {
        Ok(ModelResponse {
            content: None,
            tool_calls: Vec::new(),
        })
    }
}

struct InertTransport;

impl Transport for InertTransport {
    fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
        Ok(ResponseResult::Ok {})
    }
}

pub(crate) struct TestWorkerHarness {
    worker: Option<Worker>,
    dir: PathBuf,
    room: RoomId,
    agent: AgentId,
}

impl TestWorkerHarness {
    /// Opens one room with one agent. The Human grants every implemented Orchestrator
    /// capability so harness operations exercise settlement rather than grant policy.
    pub(crate) fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bus-test-harness-{}-{}-{}",
            std::process::id(),
            crate::bus::io::now_ns(),
            NEXT_HARNESS_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let mut worker = Self::open(&dir);
        let mut state = worker.state.clone();
        let room = state.create_room("room").unwrap();
        let agent = state
            .create_agent(room, "agent", Provider::Codex, dir.clone(), None)
            .unwrap();
        for capability in [
            Capability::InspectRoom,
            Capability::Coordinate,
            Capability::AbandonIdleRequest,
            Capability::PersistWorkflowDraft,
            Capability::PromoteWorkflowDraft,
            Capability::ManageResourceLease,
            Capability::ApprovePermissionOnce,
        ] {
            state
                .orchestrator_state_mut()
                .grant(
                    room,
                    ParticipantId::Human,
                    ParticipantId::Orchestrator,
                    capability,
                )
                .unwrap();
        }
        worker.save(state).unwrap();
        Self {
            worker: Some(worker),
            dir,
            room,
            agent,
        }
    }

    fn open(dir: &Path) -> Worker {
        let mut worker = Worker::open(dir.to_owned(), Box::new(InertTransport)).unwrap();
        worker
            .install_test_orchestrator(DormantModel, dir.to_owned())
            .unwrap();
        worker
    }

    pub(crate) fn room(&self) -> RoomId {
        self.room
    }

    pub(crate) fn agent(&self) -> AgentId {
        self.agent
    }

    /// Applies one command with the coordinator loop's result bookkeeping.
    pub(crate) fn command(&mut self, command: BusCommand) -> Result<(), String> {
        let worker = self.worker_mut();
        let (events, _) = mpsc::channel();
        let result = worker.command(command, &events);
        worker.last_command_id += 1;
        if let Err(error) = &result {
            worker.error = Some(error.clone());
        }
        result
    }

    pub(crate) fn orchestrator_operation(
        &mut self,
        operation: RoomOperation,
    ) -> Result<Value, String> {
        let room = self.room;
        self.worker_mut()
            .settle_test_orchestrator_operation(room, operation)
    }

    pub(crate) fn snapshot(&self) -> BusSnapshot {
        self.worker
            .as_ref()
            .expect("harness worker is open")
            .snapshot()
    }

    /// Reopens the same durable document with a freshly installed test orchestrator.
    pub(crate) fn restart(&mut self) {
        drop(self.worker.take());
        self.worker = Some(Self::open(&self.dir));
    }

    fn worker_mut(&mut self) -> &mut Worker {
        self.worker.as_mut().expect("harness worker is open")
    }
}

impl Drop for TestWorkerHarness {
    fn drop(&mut self) {
        drop(self.worker.take());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
