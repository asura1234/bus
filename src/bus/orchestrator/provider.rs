use std::{
    fmt,
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ModelRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ModelMessage {
    pub(crate) role: ModelRole,
    pub(crate) content: String,
}

#[cfg(test)]
impl ModelMessage {
    pub(crate) fn user(content: &str) -> Self {
        Self {
            role: ModelRole::User,
            content: content.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ModelRequest {
    pub(crate) model: String,
    pub(crate) temperature: f64,
    pub(crate) thinking: ThinkingMode,
    pub(crate) messages: Vec<ModelMessage>,
    pub(crate) tools: Vec<Value>,
    pub(crate) tool_choice: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ThinkingMode {
    #[serde(rename = "type")]
    pub(crate) kind: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrchestratorModel {
    DeepSeekV41Flash,
}

impl OrchestratorModel {
    pub(crate) const fn wire_name(self) -> &'static str {
        match self {
            Self::DeepSeekV41Flash => "deepseek-flash",
        }
    }
}

impl ModelRequest {
    #[cfg(test)]
    pub(crate) fn for_test(messages: Vec<ModelMessage>) -> Self {
        Self::new(OrchestratorModel::DeepSeekV41Flash, messages)
    }

    pub(crate) fn new(model: OrchestratorModel, messages: Vec<ModelMessage>) -> Self {
        let registry = crate::bus::orchestrator::OrchestratorToolRegistry;
        Self {
            model: model.wire_name().into(),
            temperature: 0.0,
            thinking: ThinkingMode {
                kind: "disabled".into(),
            },
            messages,
            tools: registry
                .schemas()
                .iter()
                .map(|schema| schema.provider_definition())
                .collect(),
            tool_choice: "auto".into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProviderError {
    Authentication,
    RateLimited,
    Unavailable,
    InvalidResponse,
    Cancelled,
}

#[derive(Clone, Default)]
pub(crate) struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub(crate) trait ModelAdapter {
    fn complete(
        &self,
        request: ModelRequest,
        credential: crate::bus::credentials::CredentialGeneration,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<ModelResponse, ProviderError>> + Send;
}

#[derive(Clone)]
pub(crate) struct DeepSeekAdapter {
    endpoint: String,
    client: reqwest::Client,
}

impl Default for DeepSeekAdapter {
    fn default() -> Self {
        Self {
            endpoint: "https://api.deepseek.com/chat/completions".into(),
            client: reqwest::Client::new(),
        }
    }
}

impl ModelAdapter for DeepSeekAdapter {
    async fn complete(
        &self,
        request: ModelRequest,
        credential: crate::bus::credentials::CredentialGeneration,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        if request.model != OrchestratorModel::DeepSeekV41Flash.wire_name()
            || request.temperature != 0.0
        {
            return Err(ProviderError::InvalidResponse);
        }
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(credential.expose_for_provider())
            .json(&request)
            .send()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(classify_provider_status(status, ""));
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let value = response
            .json::<Value>()
            .await
            .map_err(|_| ProviderError::InvalidResponse)?;
        normalize_provider_response(value)
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Authentication => "provider authentication failed",
            Self::RateLimited => "provider rate limited",
            Self::Unavailable => "provider unavailable",
            Self::InvalidResponse => "provider returned an invalid response",
            Self::Cancelled => "provider request cancelled",
        };
        formatter.write_str(label)
    }
}

pub(crate) fn classify_provider_status(status: u16, _body: &str) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Authentication,
        429 => ProviderError::RateLimited,
        500..=599 => ProviderError::Unavailable,
        _ => ProviderError::InvalidResponse,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProviderToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) arguments: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ModelResponse {
    pub(crate) content: Option<String>,
    pub(crate) tool_calls: Vec<ProviderToolCall>,
}

pub(crate) fn normalize_provider_response(value: Value) -> Result<ModelResponse, ProviderError> {
    let message = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .ok_or(ProviderError::InvalidResponse)?;
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(|call| {
                    let function = call.get("function").ok_or(ProviderError::InvalidResponse)?;
                    let arguments = function
                        .get("arguments")
                        .and_then(Value::as_str)
                        .ok_or(ProviderError::InvalidResponse)?;
                    Ok(ProviderToolCall {
                        id: call
                            .get("id")
                            .and_then(Value::as_str)
                            .ok_or(ProviderError::InvalidResponse)?
                            .into(),
                        name: function
                            .get("name")
                            .and_then(Value::as_str)
                            .ok_or(ProviderError::InvalidResponse)?
                            .into(),
                        arguments: serde_json::from_str(arguments)
                            .map_err(|_| ProviderError::InvalidResponse)?,
                    })
                })
                .collect::<Result<Vec<_>, ProviderError>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(ModelResponse {
        content,
        tool_calls,
    })
}
