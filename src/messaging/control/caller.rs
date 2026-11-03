//! Current process facts for sends; the native owner resolves process trees.
use crate::{platform, protocol::api::schema::ReportingProcess};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SendCaller {
    /// Internal follow-ups reuse the actor of a successful interrupt receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) interrupt_request_id: Option<String>,
    pub(crate) launch_id: Option<String>,
    pub(crate) pane_id: Option<String>,
    pub(crate) agent_context: bool,
    pub(crate) bus_context: bool,
    pub(crate) process: Option<ReportingProcess>,
}

pub(super) fn capture() -> SendCaller {
    let hint = |key| std::env::var_os(key).map(|value| value.to_string_lossy().into_owned());
    let launch_id = hint("BUS_LAUNCH_ID");
    let pane_id = hint("HERDR_PANE_ID");
    let agent_context =
        launch_id.is_some() || hint("BUS_CALLBACK_DIR").is_some() || hint("HERDR_AGENT").is_some();
    let bus_context = pane_id.is_some()
        || hint("HERDR_ENV").is_some()
        || hint("HERDR_SERVER_PID").is_some()
        || hint("HERDR_PANE_RUNTIME_ID").is_some();
    let process = platform::process_instance(std::process::id()).map(|process| ReportingProcess {
        pid: process.pid,
        birth: process.birth,
    });
    SendCaller {
        interrupt_request_id: None,
        launch_id,
        pane_id,
        agent_context,
        bus_context,
        process,
    }
}
