//! Worker-owned settlement for model-authored room commands.
use super::*;
use crate::bus::{
    orchestrator::{
        Capability, ContentBundle, ContentLoader, ContentSelector, DeepSeekAdapter,
        HumanOrchestratorMessage, JournalFact, ModelAdapter, ModelLoopError, ModelLoopRequest,
        ModelLoopResult, ModelMessage, ModelRequest, ModelRole, OperationResult,
        OrchestratorCommand, OrchestratorModel, ParticipantId, RoomMessage, RoomOperation,
        RoomQuery, RoomRecipient,
    },
    workflow_drafts::WorkflowDraftStore,
};
use serde_json::{json, Value};

struct InFlightModelRequest {
    room_id: RoomId,
    wake_revision: u64,
}

pub(super) struct RoomOrchestratorRuntime {
    requests: mpsc::SyncSender<ModelLoopRequest>,
    results: mpsc::Receiver<Result<ModelLoopResult, ModelLoopError>>,
    content: ContentBundle,
    settings_path: PathBuf,
    workflow_store: WorkflowDraftStore,
    in_flight: Option<InFlightModelRequest>,
    last_requested: BTreeMap<RoomId, u64>,
}

impl RoomOrchestratorRuntime {
    pub(super) fn production(data_dir: &Path) -> Result<Option<Self>, String> {
        let settings_path = orchestrator_settings_path(data_dir);
        let settings = crate::bus::settings::load(&settings_path)?;
        if !settings.orchestrator.enabled {
            return Ok(None);
        }
        let selector = match settings.orchestrator.content_selector {
            crate::bus::settings::OrchestratorContentSetting::TestAgentLed => {
                ContentSelector::TestAgentLed
            }
            crate::bus::settings::OrchestratorContentSetting::Production => {
                ContentSelector::Production
            }
        };
        let content = ContentLoader::test_bundle()
            .load(selector)
            .map_err(|error| format!("Room orchestrator content unavailable: {error:?}"))?;
        let repository_root = std::env::current_dir()
            .map_err(|error| format!("Room orchestrator repository root unavailable: {error}"))?;
        let credentials = crate::bus::credentials::CredentialStore::new(data_dir);
        Self::spawn(
            DeepSeekAdapter::default(),
            credentials,
            content,
            settings_path,
            repository_root,
        )
        .map(Some)
    }

    fn spawn<A>(
        adapter: A,
        credentials: crate::bus::credentials::CredentialStore,
        content: ContentBundle,
        settings_path: PathBuf,
        repository_root: PathBuf,
    ) -> Result<Self, String>
    where
        A: ModelAdapter + Send + Sync + 'static,
    {
        let (requests, results) = crate::bus::orchestrator::spawn_model_loop(adapter, credentials)?;
        Ok(Self {
            requests,
            results,
            content,
            settings_path,
            workflow_store: WorkflowDraftStore::new(repository_root)
                .map_err(|error| format!("Workflow repository unavailable: {error:?}"))?,
            in_flight: None,
            last_requested: BTreeMap::new(),
        })
    }

    fn system_prompt(&self) -> Result<String, String> {
        let settings = crate::bus::settings::load(&self.settings_path)?;
        Ok(settings
            .orchestrator
            .system_prompt_override
            .filter(|prompt| !prompt.trim().is_empty())
            .unwrap_or_else(|| self.content.system.clone()))
    }
}

#[cfg(test)]
fn orchestrator_settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

#[cfg(not(test))]
fn orchestrator_settings_path(data_dir: &Path) -> PathBuf {
    crate::bus::settings::path().unwrap_or_else(|| data_dir.join("settings.json"))
}

impl Worker {
    pub(super) fn create_developer_workflow_approval(
        &mut self,
        command: crate::bus::orchestrator::CreateDeveloperWorkflowApproval,
    ) -> Result<(), String> {
        if self.state.room(command.room_id).is_none() {
            return Err("Workflow approval room is unavailable".into());
        }
        let runtime = self
            .room_orchestrator
            .take()
            .ok_or_else(|| "Room orchestrator workflow store is unavailable".to_owned())?;
        let result = self.record_developer_workflow_approval(&runtime, command);
        // Rejected approvals re-read review evidence too, so a stale display is replaced.
        self.refresh_workflow_promotion_reviews_with(&runtime);
        self.room_orchestrator = Some(runtime);
        result
    }

    fn record_developer_workflow_approval(
        &mut self,
        runtime: &RoomOrchestratorRuntime,
        command: crate::bus::orchestrator::CreateDeveloperWorkflowApproval,
    ) -> Result<(), String> {
        let mut state = self.state.clone();
        let room_id = command.room_id;
        let approval = runtime
            .workflow_store
            .create_approval(
                state.orchestrator_state_mut().workflow_drafts_mut(),
                command,
            )
            .map_err(|error| format!("Workflow approval rejected: {error:?}"))?;
        state.orchestrator_state_mut().record_fact(
            room_id,
            JournalFact::WorkflowApprovalCreated {
                approval_id: approval.approval_id,
            },
        );
        if state == self.state {
            return Ok(());
        }
        self.save(state)
    }

    #[cfg(test)]
    pub(super) fn install_test_orchestrator<A>(
        &mut self,
        adapter: A,
        repository_root: PathBuf,
    ) -> Result<(), String>
    where
        A: ModelAdapter + Send + Sync + 'static,
    {
        let credentials = crate::bus::credentials::CredentialStore::new(&self.data_dir);
        credentials.replace("scripted-test-key")?;
        let content = ContentLoader::test_bundle()
            .load(ContentSelector::TestAgentLed)
            .map_err(|error| format!("Test content unavailable: {error:?}"))?;
        self.room_orchestrator = Some(RoomOrchestratorRuntime::spawn(
            adapter,
            credentials,
            content,
            self.data_dir.join("settings.json"),
            repository_root,
        )?);
        self.refresh_workflow_promotion_reviews();
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn settle_test_orchestrator_operation(
        &mut self,
        room_id: RoomId,
        operation: RoomOperation,
    ) -> Result<Value, String> {
        let mut runtime = self
            .room_orchestrator
            .take()
            .ok_or_else(|| "Test room orchestrator is not installed".to_owned())?;
        let result = self.settle_orchestrator_operation(&mut runtime, room_id, operation);
        self.room_orchestrator = Some(runtime);
        result
    }

    #[cfg(test)]
    pub(super) fn settle_test_orchestrator_command(
        &mut self,
        room_id: RoomId,
        command: OrchestratorCommand,
    ) -> Value {
        let Some(mut runtime) = self.room_orchestrator.take() else {
            return json!({"ok":false,"error":"Test room orchestrator is not installed"});
        };
        let result = self.settle_orchestrator_command(&mut runtime, room_id, command);
        self.room_orchestrator = Some(runtime);
        result
    }

    pub(super) fn message_orchestrator(
        &mut self,
        message: HumanOrchestratorMessage,
    ) -> Result<(), String> {
        if self.room_orchestrator.is_none() {
            return Err("Room orchestrator is disabled; the message was not recorded".into());
        }
        let room_id = message.room_id;
        let mut state = self.state.clone();
        let message_id = state
            .record_room_message(
                room_id,
                RoomMessage {
                    author: ParticipantId::Human,
                    to: RoomRecipient::Orchestrator,
                    text: message.body,
                    work: None,
                },
            )
            .map_err(|error| error.to_string())?;
        state
            .orchestrator_state_mut()
            .record_fact(room_id, JournalFact::HumanMessage { message_id });
        self.save(state)
    }

    pub(super) fn refresh_workflow_promotion_reviews(&mut self) {
        if let Some(runtime) = self.room_orchestrator.take() {
            self.refresh_workflow_promotion_reviews_with(&runtime);
            self.room_orchestrator = Some(runtime);
        }
    }

    fn refresh_workflow_promotion_reviews_with(&mut self, runtime: &RoomOrchestratorRuntime) {
        let reviews = runtime
            .workflow_store
            .promotion_reviews(self.state.orchestrator_state().workflow_drafts());
        if self.state.set_workflow_promotion_reviews(reviews) {
            self.revision += 1;
        }
    }

    pub(super) fn drive_orchestrator(&mut self) -> Result<(), String> {
        let Some(mut runtime) = self.room_orchestrator.take() else {
            return Ok(());
        };
        let result = self.drive_orchestrator_runtime(&mut runtime);
        self.room_orchestrator = Some(runtime);
        result
    }

    fn drive_orchestrator_runtime(
        &mut self,
        runtime: &mut RoomOrchestratorRuntime,
    ) -> Result<(), String> {
        if let Some(in_flight) = runtime.in_flight.take() {
            match runtime.results.try_recv() {
                Ok(Ok(result)) => {
                    let had_commands = !result.commands.is_empty();
                    let mut settled = Vec::with_capacity(result.commands.len());
                    for command in result.commands {
                        settled.push(self.settle_orchestrator_command(
                            runtime,
                            in_flight.room_id,
                            command,
                        ));
                    }
                    let result_digest = crate::bus::io::digest(
                        serde_json::to_string(&settled)
                            .map_err(|error| error.to_string())?
                            .as_bytes(),
                    );
                    let mut state = self.state.clone();
                    state.orchestrator_state_mut().record_fact(
                        in_flight.room_id,
                        JournalFact::ProviderActivity {
                            participant: ParticipantId::Orchestrator,
                            turn: None,
                            digest: result_digest,
                        },
                    );
                    self.save(state)?;
                    runtime
                        .last_requested
                        .insert(in_flight.room_id, in_flight.wake_revision);
                    if had_commands {
                        let wake_revision = self
                            .state
                            .orchestrator_state()
                            .wake_revision(in_flight.room_id);
                        let request =
                            self.orchestrator_request(runtime, in_flight.room_id, Some(&settled))?;
                        runtime
                            .requests
                            .try_send(ModelLoopRequest {
                                request,
                                cancel: Default::default(),
                            })
                            .map_err(|error| format!("Room model queue unavailable: {error}"))?;
                        runtime
                            .last_requested
                            .insert(in_flight.room_id, wake_revision);
                        runtime.in_flight = Some(InFlightModelRequest {
                            room_id: in_flight.room_id,
                            wake_revision,
                        });
                        return Ok(());
                    }
                }
                Ok(Err(error)) => {
                    runtime
                        .last_requested
                        .insert(in_flight.room_id, in_flight.wake_revision);
                    self.error = Some(format!("Room orchestrator provider failed: {error:?}"));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    runtime.in_flight = Some(in_flight);
                    return Ok(());
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("Room orchestrator model loop disconnected".into())
                }
            }
        }

        let candidate = self.state.rooms().find_map(|room| {
            let wake_revision = self.state.orchestrator_state().wake_revision(room.id);
            (runtime.last_requested.get(&room.id).copied().unwrap_or(0) < wake_revision)
                .then_some((room.id, wake_revision))
        });
        let Some((room_id, wake_revision)) = candidate else {
            return Ok(());
        };
        let request = self.orchestrator_request(runtime, room_id, None)?;
        runtime
            .requests
            .try_send(ModelLoopRequest {
                request,
                cancel: Default::default(),
            })
            .map_err(|error| format!("Room model queue unavailable: {error}"))?;
        runtime.last_requested.insert(room_id, wake_revision);
        runtime.in_flight = Some(InFlightModelRequest {
            room_id,
            wake_revision,
        });
        Ok(())
    }

    fn orchestrator_request(
        &self,
        runtime: &RoomOrchestratorRuntime,
        room_id: RoomId,
        settled: Option<&[Value]>,
    ) -> Result<ModelRequest, String> {
        let context = self.state.orchestrator_state().context(room_id);
        let mut messages = vec![
            ModelMessage {
                role: ModelRole::System,
                content: runtime.system_prompt()?,
            },
            ModelMessage {
                role: ModelRole::System,
                content: runtime.content.agent.clone(),
            },
            ModelMessage {
                role: ModelRole::User,
                content: runtime.content.index.clone(),
            },
            ModelMessage {
                role: ModelRole::User,
                content: format!(
                    "ROOM_FACTS room_id={} {}",
                    room_id.0,
                    serde_json::to_string(&context).map_err(|error| error.to_string())?
                ),
            },
        ];
        if let Some(settled) = settled {
            messages.push(ModelMessage {
                role: ModelRole::User,
                content: format!(
                    "TOOL_RESULTS {}",
                    serde_json::to_string(settled).map_err(|error| error.to_string())?
                ),
            });
        }
        Ok(ModelRequest::new(
            OrchestratorModel::DeepSeekV41Flash,
            messages,
        ))
    }

    fn settle_orchestrator_command(
        &mut self,
        runtime: &mut RoomOrchestratorRuntime,
        room_id: RoomId,
        command: OrchestratorCommand,
    ) -> Value {
        match command {
            OrchestratorCommand::Query(query) => {
                let kind = query_kind(&query);
                match self.settle_orchestrator_query(runtime, room_id, query) {
                    Ok(result) => json!({"kind":kind,"ok":true,"result":result}),
                    Err(error) => json!({"kind":kind,"ok":false,"error":error}),
                }
            }
            OrchestratorCommand::Operation(operation) => {
                let kind = operation_kind(&operation);
                match self.settle_orchestrator_operation(runtime, room_id, operation) {
                    Ok(result) => json!({"kind":kind,"ok":true,"result":result}),
                    Err(error) => json!({"kind":kind,"ok":false,"error":error}),
                }
            }
        }
    }

    fn settle_orchestrator_query(
        &mut self,
        runtime: &RoomOrchestratorRuntime,
        room_id: RoomId,
        query: RoomQuery,
    ) -> Result<Value, String> {
        if !self.state.orchestrator_state().authorized(
            room_id,
            ParticipantId::Orchestrator,
            Capability::InspectRoom,
        ) {
            return Err("Orchestrator lacks InspectRoom capability".into());
        }
        match query {
            RoomQuery::InspectWork { work_id } => {
                let settlements = self
                    .state
                    .requests()
                    .filter(|request| {
                        request.room_id == room_id && request.prompt.work_id == Some(work_id)
                    })
                    .filter_map(|request| self.state.work_settlement(request.id))
                    .collect::<Vec<_>>();
                if settlements.is_empty() {
                    Err("Work observation target is unknown in this room".into())
                } else {
                    Ok(json!({"work_id":work_id.0,"settlements":settlements}))
                }
            }
            RoomQuery::WaitForChange {
                after_revision,
                timeout_ms,
            } => {
                let current = self.state.orchestrator_state().wake_revision(room_id);
                Ok(json!({
                    "after_revision":after_revision,
                    "current_revision":current,
                    "changed":current > after_revision,
                    "timeout_ms":timeout_ms,
                }))
            }
            RoomQuery::ReadContent { kind, name } => {
                let body = if kind == "system" && name == "system" {
                    runtime.system_prompt()?
                } else {
                    runtime
                        .content
                        .read(&kind, &name)
                        .map(str::to_owned)
                        .map_err(|error| format!("Content read rejected: {error:?}"))?
                };
                Ok(json!({"kind":kind,"name":name,"body":body}))
            }
            RoomQuery::ReadAgent {
                agent_id,
                selection,
            } => {
                if self
                    .state
                    .agent(agent_id)
                    .is_none_or(|agent| agent.room_id != room_id)
                {
                    return Err("Agent observation target is outside this room".into());
                }
                match selection {
                    crate::bus::orchestrator::AgentTerminalReadSelection::VisibleViewport => {
                        self.dev_read(agent_id, Some("visible"), None)
                    }
                    crate::bus::orchestrator::AgentTerminalReadSelection::RecentTail { lines } => {
                        self.dev_read(agent_id, Some("recent"), Some(lines.get()))
                    }
                }
            }
            RoomQuery::ObservePermissionPrompt { agent_id } => {
                if self
                    .state
                    .agent(agent_id)
                    .is_none_or(|agent| agent.room_id != room_id)
                {
                    return Err("Permission observation target is outside this room".into());
                }
                self.permission_candidate(agent_id)
                    .map(super::dev_control::permission_candidate_json)
            }
            RoomQuery::ReadWorkflowDraft {
                draft_id,
                max_bytes,
            } => runtime
                .workflow_store
                .read(
                    self.state.orchestrator_state().workflow_drafts(),
                    room_id,
                    &draft_id,
                    max_bytes,
                )
                .map(|(record, markdown)| {
                    json!({
                        "draft_id":record.draft_id,
                        "revision":record.revision,
                        "content_digest":record.content_digest,
                        "markdown":markdown,
                    })
                })
                .map_err(|error| format!("Workflow draft read rejected: {error:?}")),
        }
    }

    fn settle_orchestrator_operation(
        &mut self,
        runtime: &mut RoomOrchestratorRuntime,
        room_id: RoomId,
        operation: RoomOperation,
    ) -> Result<Value, String> {
        let capability = operation_capability(&operation);
        // Grants start at Human Room Brief confirmation, so the unlocked room's proposal is the
        // sole bootstrap operation; it still passes the exact-revision/nonblank validation.
        let bootstrap = matches!(operation, RoomOperation::ProposeRoomBrief { .. })
            && self
                .state
                .room(room_id)
                .is_some_and(|room| !room.brief.locked);
        if !bootstrap
            && !self.state.orchestrator_state().authorized(
                room_id,
                ParticipantId::Orchestrator,
                capability,
            )
        {
            return Err(format!("Orchestrator lacks {capability:?} capability"));
        }
        if let RoomOperation::ApprovePermissionOnce(grant) = &operation {
            return self.approve_permission_once(ParticipantId::Orchestrator, None, grant.clone());
        }
        let refreshes_reviews = matches!(
            operation,
            RoomOperation::PersistWorkflowDraft(_) | RoomOperation::PromoteWorkflowDraft(_)
        );
        let kind = operation_kind(&operation);
        let intent_digest = crate::bus::io::digest(
            serde_json::to_vec(&operation)
                .map_err(|error| error.to_string())?
                .as_slice(),
        );
        let mut intent_state = self.state.clone();
        let intent = intent_state
            .orchestrator_state_mut()
            .begin_operation(room_id, ParticipantId::Orchestrator, kind, &intent_digest)
            .map_err(|error| format!("Operation intent rejected: {error:?}"))?;
        self.save(intent_state)?;

        let mut settled_state = self.state.clone();
        let outcome = match operation {
            RoomOperation::SendMessage(message) => (|| {
                if message.author != ParticipantId::Orchestrator {
                    Err("Model-authored message has an invalid author".into())
                } else {
                    let agent_id = match message.to.clone() {
                        RoomRecipient::Agent(agent_id) => agent_id,
                        RoomRecipient::Human => {
                            let message_id = settled_state
                                .record_room_message(room_id, message)
                                .map_err(|error| format!("Room message rejected: {error:?}"))?;
                            return Ok(json!({"message_id":message_id.0}));
                        }
                        RoomRecipient::Orchestrator => {
                            return Err("The Orchestrator cannot address itself".into());
                        }
                    };
                    let request_ids = settled_state
                        .submit_message_from(
                            room_id,
                            Draft {
                                text: message.text,
                                files: Vec::new(),
                                recipient_ids: [agent_id].into_iter().collect(),
                            },
                            ParticipantId::Orchestrator,
                            message.work,
                            crate::bus::io::now_ms(),
                        )
                        .map_err(|error| format!("Message delegation rejected: {error:?}"))?;
                    let message_id = settled_state
                        .request(request_ids[0])
                        .ok_or_else(|| "Delegated request missing after submission".to_owned())?
                        .prompt
                        .id;
                    Ok(json!({
                        "message_id":message_id.0,
                        "request_ids":request_ids,
                        "work_id":settled_state
                            .request(request_ids[0])
                            .and_then(|request| request.prompt.work_id)
                            .map(|work| work.0),
                    }))
                }
            })(),
            RoomOperation::ProposeRoomBrief {
                expected_revision,
                goal,
                non_goals,
            } => settled_state
                .propose_room_brief(room_id, expected_revision, goal, non_goals)
                .map_err(|error| format!("Room Brief proposal rejected: {error:?}"))
                .and_then(|revision| {
                    let (_, proposal_digest) = settled_state
                        .room_brief_proposal(room_id)
                        .ok_or_else(|| "Room Brief proposal missing after settlement".to_owned())?;
                    Ok(json!({"revision":revision,"proposal_digest":proposal_digest}))
                }),
            RoomOperation::AbandonIdleRequest {
                request_id,
                expected_agent,
                expected_incarnation,
                expected_semantic_revision,
                expected_turn,
            } => (|| {
                if expected_incarnation != 1 {
                    return Err("Request recovery incarnation is stale".into());
                }
                if settled_state
                    .orchestrator_state()
                    .wake_revision(room_id)
                    != expected_semantic_revision
                {
                    return Err("Request recovery semantic revision is stale".into());
                }
                let request = settled_state
                    .request(request_id)
                    .ok_or_else(|| "Request recovery target is unknown".to_owned())?;
                if request.room_id != room_id || request.agent_id != expected_agent {
                    return Err("Request recovery target no longer matches room facts".into());
                }
                if request.provider_turn_id != expected_turn {
                    return Err("Request recovery provider turn is stale".into());
                }
                settled_state
                    .recover_idle_request(request_id, crate::bus::io::now_ms())
                    .map_err(|error| format!("Request recovery rejected: {error:?}"))?;
                Ok(json!({
                    "request_id":request_id.0,
                    "agent_id":expected_agent.0,
                    "stage":"abandoned",
                }))
            })(),
            RoomOperation::PersistWorkflowDraft(mutation) => runtime
                .workflow_store
                .persist(
                    settled_state
                        .orchestrator_state_mut()
                        .workflow_drafts_mut(),
                    room_id,
                    mutation,
                )
                .map(|receipt| {
                    json!({
                        "draft_id":receipt.draft_id,
                        "revision":receipt.revision,
                        "content_digest":receipt.content_digest,
                        "derived_target":receipt.derived_target,
                    })
                })
                .map_err(|error| format!("Workflow draft rejected: {error:?}")),
            RoomOperation::PromoteWorkflowDraft(promotion) => runtime
                .workflow_store
                .promote(
                    settled_state
                        .orchestrator_state_mut()
                        .workflow_drafts_mut(),
                    room_id,
                    promotion,
                )
                .map(|receipt| {
                    json!({
                        "approval_id":receipt.approval_id,
                        "content_digest":receipt.content_digest,
                        "derived_target":receipt.derived_target,
                    })
                })
                .map_err(|error| format!("Workflow promotion rejected: {error:?}")),
            RoomOperation::AcquireResource { resource, ttl_ms } => settled_state
                .orchestrator_state_mut()
                .acquire_resource(
                    room_id,
                    ParticipantId::Orchestrator,
                    &resource,
                    ttl_ms,
                    crate::bus::io::now_ms(),
                )
                .map(|lease| {
                    json!({"lease_id":lease.lease_id.0,"expires_at_ms":lease.expires_at_ms})
                })
                .map_err(|error| format!("Resource lease rejected: {error:?}")),
            RoomOperation::ReleaseResource { lease_id } => settled_state
                .orchestrator_state_mut()
                .release_resource(room_id, ParticipantId::Orchestrator, lease_id)
                .map(|()| json!({"released":true}))
                .map_err(|error| format!("Resource release rejected: {error:?}")),
            RoomOperation::ApprovePermissionOnce(_) => {
                unreachable!("permission approval settles through its audited path above")
            }
        };

        let settled = match outcome {
            Ok(result) => {
                let receipt_digest = crate::bus::io::digest(
                    serde_json::to_string(&result)
                        .map_err(|error| error.to_string())?
                        .as_bytes(),
                );
                settled_state
                    .orchestrator_state_mut()
                    .reconcile_operation(
                        intent.operation_id,
                        OperationResult::Applied { receipt_digest },
                    )
                    .map_err(|error| format!("Operation settlement failed: {error:?}"))?;
                self.save(settled_state)?;
                Ok(json!({"operation_id":intent.operation_id.0,"receipt":result}))
            }
            Err(error) => {
                settled_state
                    .orchestrator_state_mut()
                    .reconcile_operation(
                        intent.operation_id,
                        OperationResult::Rejected {
                            code: error.clone(),
                        },
                    )
                    .map_err(|state_error| {
                        format!("Operation rejection settlement failed: {state_error:?}")
                    })?;
                self.save(settled_state)?;
                Err(error)
            }
        };
        if refreshes_reviews {
            self.refresh_workflow_promotion_reviews_with(runtime);
        }
        settled
    }
}

fn query_kind(query: &RoomQuery) -> &'static str {
    match query {
        RoomQuery::InspectWork { .. } => "inspect_work",
        RoomQuery::WaitForChange { .. } => "wait_for_change",
        RoomQuery::ReadAgent { .. } => "read_agent",
        RoomQuery::ObservePermissionPrompt { .. } => "observe_permission_prompt",
        RoomQuery::ReadWorkflowDraft { .. } => "read_workflow_draft",
        RoomQuery::ReadContent { .. } => "read_content",
    }
}

fn operation_kind(operation: &RoomOperation) -> &'static str {
    match operation {
        RoomOperation::SendMessage(_) => "send_message",
        RoomOperation::ProposeRoomBrief { .. } => "propose_room_brief",
        RoomOperation::AbandonIdleRequest { .. } => "abandon_idle_request",
        RoomOperation::PersistWorkflowDraft(_) => "persist_workflow_draft",
        RoomOperation::PromoteWorkflowDraft(_) => "promote_workflow_draft",
        RoomOperation::AcquireResource { .. } => "acquire_resource",
        RoomOperation::ReleaseResource { .. } => "release_resource",
        RoomOperation::ApprovePermissionOnce(_) => "approve_permission_once",
    }
}

fn operation_capability(operation: &RoomOperation) -> Capability {
    match operation {
        RoomOperation::SendMessage(_) | RoomOperation::ProposeRoomBrief { .. } => {
            Capability::Coordinate
        }
        RoomOperation::AbandonIdleRequest { .. } => Capability::AbandonIdleRequest,
        RoomOperation::PersistWorkflowDraft(_) => Capability::PersistWorkflowDraft,
        RoomOperation::PromoteWorkflowDraft(_) => Capability::PromoteWorkflowDraft,
        RoomOperation::AcquireResource { .. } | RoomOperation::ReleaseResource { .. } => {
            Capability::ManageResourceLease
        }
        RoomOperation::ApprovePermissionOnce(_) => Capability::ApprovePermissionOnce,
    }
}
