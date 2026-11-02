//! Locate callers by owned PTY process incarnations, without reading their env.
use crate::platform::{self, ProcessInstance};
use crate::protocol::api::schema::{CallerLocation, ReportingProcess, ResponseResult};
use crate::server::api::errors::{encode_error, encode_success};
use crate::server::app::App;

impl App {
    pub(in crate::server::api) fn handle_pane_locate_process(
        &self,
        id: String,
        process: ReportingProcess,
    ) -> String {
        let caller = ProcessInstance {
            pid: process.pid,
            birth: process.birth,
        };
        match self.locate_caller(caller) {
            Some(location) => encode_success(id, ResponseResult::CallerLocation { location }),
            None => encode_error(
                id,
                "caller_unknown",
                "Caller process identity is unavailable or stale",
            ),
        }
    }

    fn locate_caller(&self, caller: ProcessInstance) -> Option<CallerLocation> {
        let ancestry = platform::process_ancestry(caller.pid);
        if ancestry.first() != Some(&caller) {
            return None;
        }
        let mut unverifiable_root = false;
        for ws in 0..self.state.workspaces.len() {
            for pane in self.state.pane_ids_for_workspace(ws) {
                let terminal = self.state.terminal_id_for_pane(ws, pane)?;
                let Some(runtime) = self.terminal_runtimes.get(&terminal) else {
                    continue;
                };
                let Some(root) = runtime.child_process_instance() else {
                    unverifiable_root = true;
                    continue;
                };
                if !ancestry.contains(&root) {
                    // Reparented children can retain the pane's PTY session.
                    // Session IDs alone cannot prove the original incarnation,
                    // so refuse rather than classify such a caller as developer.
                    if platform::session_processes(root.pid).contains(&caller.pid) {
                        return None;
                    }
                    continue;
                }
                let terminal = self.state.terminals.get(&terminal)?;
                return Some(CallerLocation {
                    pane: Some(self.pane_info(ws, pane)?),
                    agent_owned: terminal.is_agent_terminal() || terminal.native_resume.is_some(),
                });
            }
        }
        // An unreadable PTY root cannot prove a caller is outside that pane.
        if unverifiable_root {
            return None;
        }
        // Every owned PTY starts below this native server. Reaching one of
        // the server's own ancestors without finding a PTY root proves the
        // caller branched outside those panes. Higher protected OS processes
        // need not be readable (for example macOS login/launchd).
        let server_ancestry = platform::process_ancestry(std::process::id());
        let outside_panes = ancestry.iter().any(|p| server_ancestry.contains(p));
        let complete = outside_panes
            || ancestry.last().is_some_and(|last| {
                last.pid == 1
                    || platform::process_parent(last.pid)
                        .is_some_and(|pid| pid == 1 || !platform::process_exists(pid))
            })
            // A detached server may share only protected OS ancestors with an
            // external client. Once this readable prefix reaches a process
            // older than the server, no unseen ancestor can be one of its PTYs.
            // macOS uses a separate monotonic stamp, never the opaque birth key.
            || ancestry.last().zip(server_ancestry.first()).is_some_and(|(ancestor, server)| {
                platform::caller_precedes_server(*ancestor, *server) == Some(true)
            });
        complete.then_some(CallerLocation {
            pane: None,
            agent_owned: false,
        })
    }
}
