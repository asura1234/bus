use std::sync::mpsc;

use super::{
    CancellationToken, ModelAdapter, ModelRequest, OrchestratorCommand, OrchestratorToolRegistry,
    ProviderError, ToolPolicyError,
};

pub(crate) struct ModelLoopRequest {
    pub(crate) request: ModelRequest,
    pub(crate) cancel: CancellationToken,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ModelLoopError {
    CredentialUnavailable,
    Provider(ProviderError),
    Policy(ToolPolicyError),
}

pub(crate) struct ModelLoopResult {
    pub(crate) commands: Vec<OrchestratorCommand>,
}

pub(crate) type ModelLoopRequests = mpsc::SyncSender<ModelLoopRequest>;
pub(crate) type ModelLoopResults = mpsc::Receiver<Result<ModelLoopResult, ModelLoopError>>;

/// Starts only the provider/tool-decode half. The Bus Worker remains the sole
/// command executor and durable writer; decoded commands return through the
/// channel in provider order for sequential settlement.
pub(crate) fn spawn_model_loop<A>(
    adapter: A,
    credentials: crate::bus::credentials::CredentialStore,
) -> Result<(ModelLoopRequests, ModelLoopResults), String>
where
    A: ModelAdapter + Send + Sync + 'static,
{
    let (requests_tx, requests_rx) = mpsc::sync_channel::<ModelLoopRequest>(16);
    let (results_tx, results_rx) = mpsc::sync_channel(16);
    std::thread::Builder::new()
        .name("bus-room-model".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("room model runtime");
            while let Ok(invocation) = requests_rx.recv() {
                let result = (|| {
                    let credential = credentials
                        .load()
                        .ok_or(ModelLoopError::CredentialUnavailable)?;
                    let response = runtime
                        .block_on(adapter.complete(
                            invocation.request,
                            credential,
                            invocation.cancel,
                        ))
                        .map_err(ModelLoopError::Provider)?;
                    let registry = OrchestratorToolRegistry;
                    let commands = response
                        .tool_calls
                        .into_iter()
                        .map(|call| {
                            registry
                                .decode(super::ModelToolCall {
                                    name: call.name,
                                    arguments: call.arguments,
                                })
                                .map_err(ModelLoopError::Policy)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(ModelLoopResult { commands })
                })();
                if results_tx.send(result).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| error.to_string())?;
    Ok((requests_tx, results_rx))
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        sync::{Arc, Mutex},
    };

    use serde_json::json;

    use super::*;
    use crate::bus::orchestrator::{ModelResponse, ProviderToolCall, RoomQuery};

    #[derive(Clone)]
    struct FakeAdapter {
        calls: Arc<Mutex<Vec<(std::thread::ThreadId, String, u64)>>>,
    }

    impl ModelAdapter for FakeAdapter {
        fn complete(
            &self,
            request: ModelRequest,
            credential: crate::bus::credentials::CredentialGeneration,
            _cancel: CancellationToken,
        ) -> impl Future<Output = Result<ModelResponse, ProviderError>> + Send {
            let calls = Arc::clone(&self.calls);
            async move {
                calls.lock().unwrap().push((
                    std::thread::current().id(),
                    request.model,
                    credential.generation,
                ));
                Ok(ModelResponse {
                    content: None,
                    tool_calls: vec![ProviderToolCall {
                        id: "call-1".into(),
                        name: "inspect_work".into(),
                        arguments: json!({"work_id": 7}),
                    }],
                })
            }
        }
    }

    #[test]
    fn room_orchestrator_core_model_loop_is_off_worker_and_only_returns_decoded_commands() {
        let root =
            std::env::temp_dir().join(format!("bus-model-loop-{}", crate::bus::io::now_ns()));
        std::fs::create_dir_all(&root).unwrap();
        let credentials = crate::bus::credentials::CredentialStore::new(&root);
        credentials.replace("fake-key-only").unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let adapter = FakeAdapter {
            calls: Arc::clone(&calls),
        };
        let worker_thread = std::thread::current().id();
        let (requests, results) = spawn_model_loop(adapter, credentials).unwrap();
        requests
            .send(ModelLoopRequest {
                request: ModelRequest::for_test(vec![]),
                cancel: CancellationToken::default(),
            })
            .unwrap();
        let result = results.recv().unwrap().unwrap();
        assert!(matches!(
            result.commands.as_slice(),
            [OrchestratorCommand::Query(RoomQuery::InspectWork { .. })]
        ));
        let observations = calls.lock().unwrap();
        assert_ne!(observations[0].0, worker_thread);
        assert_eq!(observations[0].1, "deepseek-flash");
        assert_eq!(observations[0].2, 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
