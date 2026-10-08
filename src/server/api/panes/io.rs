use super::pane_not_found;
use crate::protocol::api::schema::{
    PaneInputSetParams, PaneLinkActivateParams, PaneReadParams, PaneReadResult,
    PaneSendInputParams, PaneSendKeysParams, PaneSendTextParams, ResponseResult,
};
use crate::server::api::errors::{encode_error, encode_success};
use crate::server::api::input_encoding::encode_api_keys;
use crate::server::app::App;
use bytes::Bytes;

impl App {
    pub(in crate::server::api) fn handle_pane_link_activate(
        &mut self,
        id: String,
        params: PaneLinkActivateParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return encode_error(id, "pane_not_found", "pane not found");
        };
        if !self.state.pane_visible_on_active_surface(ws_idx, pane_id) {
            return encode_error(id, "stale_target", "pane is no longer visible");
        }
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return encode_error(id, "pane_not_found", "pane runtime not found");
        };
        let current_offset = runtime
            .scroll_metrics()
            .map(|metrics| metrics.offset_from_bottom as u64);
        if params
            .offset_from_bottom
            .is_some_and(|expected| current_offset != Some(expected))
        {
            return encode_error(
                id,
                "stale_content",
                "pane viewport changed before link activation",
            );
        }
        let content_revision = runtime.content_seq();
        if content_revision % 2 != 0
            || params
                .content_revision
                .is_some_and(|expected| expected != content_revision)
        {
            return encode_error(
                id,
                "stale_content",
                "pane content changed before link activation",
            );
        }
        let url = self.state.url_at_pane_surface_cell(
            &self.terminal_runtimes,
            ws_idx,
            pane_id,
            params.viewport_row,
            params.col,
        );
        if runtime.content_seq() != content_revision
            || runtime
                .scroll_metrics()
                .map(|metrics| metrics.offset_from_bottom as u64)
                != current_offset
        {
            return encode_error(
                id,
                "stale_content",
                "pane content or viewport changed during link activation",
            );
        }
        // The client opens the resolved URL itself.
        encode_success(
            id,
            ResponseResult::PaneLinkActivated {
                url,
                handled: false,
            },
        )
    }

    pub(in crate::server::api) fn handle_pane_input_set(
        &mut self,
        id: String,
        params: PaneInputSetParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(pane) = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .and_then(|workspace| workspace.pane_state_mut(pane_id))
        else {
            return pane_not_found(id, &params.pane_id);
        };
        pane.right_click_passthrough = matches!(
            params.right_click,
            crate::protocol::api::schema::PaneRightClickTarget::Pane
        );
        encode_success(id, ResponseResult::Ok {})
    }

    pub(in crate::server::api) fn handle_pane_read(
        &mut self,
        id: String,
        params: PaneReadParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(public_pane_id) = self.public_pane_id(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some((pane, workspace_id)) = self.lookup_runtime(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(tab_idx) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.find_tab_index_for_pane(pane_id))
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let snapshot = crate::server::api::input_encoding::read_terminal_snapshot(
            pane,
            params.source,
            params.format,
            params.lines,
        );

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: public_pane_id,
                    workspace_id,
                    tab_id: self.public_tab_id(ws_idx, tab_idx).unwrap(),
                    source: params.source,
                    format: params.format,
                    text: snapshot.text,
                    revision: snapshot.revision,
                    truncated: snapshot.truncated,
                    viewport_rows: snapshot.viewport_rows,
                    viewport_columns: snapshot.viewport_columns,
                    requested_lines: snapshot.requested_lines,
                    returned_lines: snapshot.returned_lines,
                    available_lines: snapshot.available_lines,
                    exhausted: snapshot.exhausted,
                },
            },
        )
    }

    pub(in crate::server::api) fn handle_pane_send_text(
        &mut self,
        id: String,
        params: PaneSendTextParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(params.text)) {
            return encode_error(id, "pane_send_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }

    pub(in crate::server::api) fn handle_pane_send_input(
        &mut self,
        id: String,
        params: PaneSendInputParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let bytes = match crate::server::api::input_encoding::encode_api_input(
            runtime,
            &params.text,
            &params.keys,
        ) {
            Ok(bytes) => bytes,
            Err(key) => return encode_error(id, "invalid_key", format!("unsupported key {key}")),
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            return encode_error(id, "pane_send_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }

    pub(in crate::server::api) fn handle_pane_send_keys(
        &mut self,
        id: String,
        params: PaneSendKeysParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let encoded_keys = match encode_api_keys(runtime, &params.keys) {
            Ok(encoded_keys) => encoded_keys,
            Err(key) => return encode_error(id, "invalid_key", format!("unsupported key {key}")),
        };
        for bytes in encoded_keys {
            if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
                return encode_error(id, "pane_send_failed", err.to_string());
            }
        }

        encode_success(id, ResponseResult::Ok {})
    }
}
