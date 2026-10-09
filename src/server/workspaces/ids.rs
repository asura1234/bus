use crate::server::app::App;

impl App {
    pub(crate) fn find_pane(
        &self,
        pane_id: crate::utils::ids::PaneId,
    ) -> Option<(usize, &crate::server::workspaces::pane::PaneState)> {
        self.state
            .workspaces
            .iter()
            .enumerate()
            .find_map(|(ws_idx, ws)| ws.pane_state(pane_id).map(|pane| (ws_idx, pane)))
    }

    pub(crate) fn public_workspace_id(&self, ws_idx: usize) -> String {
        self.state.workspaces[ws_idx].id.clone()
    }

    pub(crate) fn public_tab_id(&self, ws_idx: usize, tab_idx: usize) -> Option<String> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab_number = ws.public_tab_number(tab_idx)?;
        Some(crate::server::workspaces::public_tab_id_for_number(
            &ws.id, tab_number,
        ))
    }

    pub(crate) fn public_pane_id(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
    ) -> Option<String> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let pane_number = ws.public_pane_number(pane_id)?;
        Some(crate::server::workspaces::public_pane_id_for_number(
            &ws.id,
            pane_number,
        ))
    }

    pub(in crate::server) fn pane_launch_env(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
        extra_env: Vec<(String, String)>,
    ) -> Option<crate::terminal::runtime::spawn::PaneLaunchEnv> {
        let workspace_id = self.public_workspace_id(ws_idx);
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab_idx = ws.find_tab_index_for_pane(pane_id)?;
        let tab_id = self.public_tab_id(ws_idx, tab_idx)?;
        let pane_id = self.public_pane_id(ws_idx, pane_id)?;
        Some(
            crate::terminal::runtime::spawn::PaneLaunchEnv::from_extra(extra_env).with_identity(
                workspace_id,
                tab_id,
                pane_id,
            ),
        )
    }

    pub(crate) fn parse_workspace_id(&self, id: &str) -> Option<usize> {
        self.state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == id)
            .or_else(|| id.strip_prefix("w_")?.parse::<usize>().ok()?.checked_sub(1))
            .or_else(|| id.parse::<usize>().ok()?.checked_sub(1))
    }

    pub(crate) fn parse_tab_id(&self, id: &str) -> Option<(usize, usize)> {
        if let Some(rest) = id.strip_prefix("t_") {
            let (ws_raw, tab_raw) = rest.rsplit_once('_')?;
            let ws_idx = self.parse_workspace_id(ws_raw)?;
            let tab_idx = tab_raw.parse::<usize>().ok()?.checked_sub(1)?;
            self.state.workspaces.get(ws_idx)?.tabs.get(tab_idx)?;
            return Some((ws_idx, tab_idx));
        }

        let (ws_raw, tab_raw) = id.rsplit_once(':')?;
        let ws_idx = self.parse_workspace_id(ws_raw)?;
        let tab_idx = if let Some(encoded) = tab_raw.strip_prefix('t') {
            let tab_number = crate::server::workspaces::decode_public_number(encoded)?;
            self.state
                .workspaces
                .get(ws_idx)?
                .tabs
                .iter()
                .position(|tab| tab.number == tab_number)?
        } else {
            tab_raw.parse::<usize>().ok()?.checked_sub(1)?
        };
        self.state.workspaces.get(ws_idx)?.tabs.get(tab_idx)?;
        Some((ws_idx, tab_idx))
    }

    fn resolve_raw_pane_id(&self, raw: u32) -> Option<crate::utils::ids::PaneId> {
        if let Some(alias) = self.state.pane_id_aliases.get(&raw).copied() {
            return self.find_pane(alias).map(|_| alias);
        }
        let pane_id = crate::utils::ids::PaneId::from_raw(raw);
        if self.find_pane(pane_id).is_some() {
            return Some(pane_id);
        }
        None
    }

    pub(crate) fn parse_pane_id(&self, id: &str) -> Option<(usize, crate::utils::ids::PaneId)> {
        if let Some(alias) = self.state.public_pane_id_aliases.get(id).copied() {
            return self.find_pane(alias).map(|(ws_idx, _)| (ws_idx, alias));
        }

        if let Some(rest) = id.strip_prefix("p_") {
            if let Some((ws_raw, pane_raw)) = rest.rsplit_once('_') {
                let ws_idx = self.parse_workspace_id(ws_raw)?;
                let pane_id = self.resolve_raw_pane_id(pane_raw.parse::<u32>().ok()?)?;
                self.state.workspaces.get(ws_idx)?.pane_state(pane_id)?;
                return Some((ws_idx, pane_id));
            }

            let pane_id = self.resolve_raw_pane_id(rest.parse::<u32>().ok()?)?;
            return self.find_pane(pane_id).map(|(ws_idx, _)| (ws_idx, pane_id));
        }

        if let Some((ws_raw, pane_number_raw)) = id.rsplit_once(":p") {
            let ws_idx = self.parse_workspace_id(ws_raw)?;
            let pane_number = crate::server::workspaces::decode_public_number(pane_number_raw)?;
            let ws = self.state.workspaces.get(ws_idx)?;
            let pane_id = ws
                .public_pane_numbers
                .iter()
                .find_map(|(pane_id, number)| (*number == pane_number).then_some(*pane_id))?;
            return Some((ws_idx, pane_id));
        }

        let (ws_raw, pane_number_raw) = id.rsplit_once('-')?;
        let ws_idx = self.parse_workspace_id(ws_raw)?;
        let pane_number = pane_number_raw.parse::<usize>().ok()?;
        let ws = self.state.workspaces.get(ws_idx)?;
        let pane_id = ws
            .public_pane_numbers
            .iter()
            .find_map(|(pane_id, number)| (*number == pane_number).then_some(*pane_id))?;
        Some((ws_idx, pane_id))
    }

    pub(crate) fn parse_current_public_pane_id(
        &self,
        id: &str,
    ) -> Option<(usize, crate::utils::ids::PaneId)> {
        let (ws_idx, pane_id) = self.parse_pane_id(id)?;
        (self.public_pane_id(ws_idx, pane_id).as_deref() == Some(id)).then_some((ws_idx, pane_id))
    }
}

use crate::server::api::input_encoding::pane_agent_status;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalTarget {
    pub ws_idx: usize,
    pub tab_idx: usize,
    pub pane_id: crate::utils::ids::PaneId,
    pub terminal_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalTargetCandidate {
    pub terminal_id: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    pub cwd: Option<String>,
    pub agent_status: crate::protocol::api::schema::AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalTargetError {
    NotFound {
        target: String,
    },
    Ambiguous {
        target: String,
        candidates: Vec<TerminalTargetCandidate>,
    },
}

impl App {
    pub(crate) fn resolve_terminal_target(
        &self,
        target: &str,
    ) -> Result<TerminalTarget, TerminalTargetError> {
        let terminal_matches: Vec<_> = self
            .terminal_targets()
            .into_iter()
            .filter(|candidate| candidate.terminal_id == target)
            .collect();
        if let Some(resolved) = self.single_terminal_match(target, terminal_matches)? {
            return Ok(resolved);
        }

        if let Some((ws_idx, pane_id)) = self.parse_current_public_pane_id(target) {
            if let Some(resolved) = self.terminal_target_for_pane(ws_idx, pane_id) {
                return Ok(resolved);
            }
        }

        let agent_matches: Vec<_> = self
            .terminal_targets()
            .into_iter()
            .filter(|candidate| {
                self.state
                    .terminals
                    .values()
                    .find(|terminal| terminal.id.to_string() == candidate.terminal_id)
                    .is_some_and(|terminal| {
                        terminal.agent_name.as_deref() == Some(target)
                            || terminal.effective_agent_label() == Some(target)
                    })
            })
            .collect();
        if let Some(resolved) = self.single_terminal_match(target, agent_matches)? {
            return Ok(resolved);
        }

        Err(TerminalTargetError::NotFound {
            target: target.to_string(),
        })
    }

    pub(crate) fn resolve_agent_target(
        &self,
        target: &str,
    ) -> Result<TerminalTarget, TerminalTargetError> {
        if let Some((ws_idx, pane_id)) = self.parse_current_public_pane_id(target) {
            if let Some(resolved) = self
                .terminal_target_for_pane(ws_idx, pane_id)
                .filter(|resolved| self.target_is_agent(resolved))
            {
                return Ok(resolved);
            }
        }

        let name_matches: Vec<_> = self
            .terminal_targets()
            .into_iter()
            .filter(|candidate| {
                self.state
                    .terminals
                    .values()
                    .find(|terminal| terminal.id.to_string() == candidate.terminal_id)
                    .is_some_and(|terminal| terminal.agent_name.as_deref() == Some(target))
            })
            .collect();
        if let Some(resolved) = self.single_terminal_match(target, name_matches)? {
            return Ok(resolved);
        }

        Err(TerminalTargetError::NotFound {
            target: target.to_string(),
        })
    }

    fn target_is_agent(&self, target: &TerminalTarget) -> bool {
        self.state
            .terminals
            .values()
            .find(|terminal| terminal.id.to_string() == target.terminal_id)
            .is_some_and(|terminal| terminal.is_agent_terminal())
    }

    fn single_terminal_match(
        &self,
        target: &str,
        matches: Vec<TerminalTarget>,
    ) -> Result<Option<TerminalTarget>, TerminalTargetError> {
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            _ => Err(TerminalTargetError::Ambiguous {
                target: target.to_string(),
                candidates: matches
                    .into_iter()
                    .filter_map(|candidate| {
                        self.terminal_target_candidate(candidate.ws_idx, candidate.pane_id)
                    })
                    .collect(),
            }),
        }
    }

    fn terminal_targets(&self) -> Vec<TerminalTarget> {
        self.state
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, ws)| {
                ws.tabs.iter().enumerate().flat_map(move |(tab_idx, tab)| {
                    tab.layout
                        .pane_ids()
                        .into_iter()
                        .filter_map(move |pane_id| {
                            tab.terminal_id(pane_id).map(|terminal_id| TerminalTarget {
                                ws_idx,
                                tab_idx,
                                pane_id,
                                terminal_id: terminal_id.to_string(),
                            })
                        })
                })
            })
            .collect()
    }

    fn terminal_target_for_pane(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
    ) -> Option<TerminalTarget> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab_idx = ws.find_tab_index_for_pane(pane_id)?;
        let terminal_id = ws.terminal_id(pane_id)?.to_string();
        Some(TerminalTarget {
            ws_idx,
            tab_idx,
            pane_id,
            terminal_id,
        })
    }

    fn terminal_target_candidate(
        &self,
        ws_idx: usize,
        pane_id: crate::utils::ids::PaneId,
    ) -> Option<TerminalTargetCandidate> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let tab_idx = ws.find_tab_index_for_pane(pane_id)?;
        let pane = ws.pane_state(pane_id)?;
        let terminal = self.state.terminals.get(&pane.attached_terminal_id)?;
        Some(TerminalTargetCandidate {
            terminal_id: terminal.id.to_string(),
            pane_id: self.public_pane_id(ws_idx, pane_id)?,
            workspace_id: self.public_workspace_id(ws_idx),
            tab_id: self.public_tab_id(ws_idx, tab_idx)?,
            cwd: ws.tabs[tab_idx]
                .cwd_for_pane(pane_id, &self.state.terminals, &self.terminal_runtimes)
                .map(|cwd| cwd.display().to_string()),
            agent_status: pane_agent_status(terminal.state, pane.seen),
        })
    }
}

use super::Workspace;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_WORKSPACE_ID: AtomicU64 = AtomicU64::new(1);
const PUBLIC_ID_ALPHABET: &[u8; 32] = b"123456789ABCDEFGHJKMNPQRSTVWXYZ0";

pub(crate) fn generate_workspace_id() -> String {
    let counter = NEXT_WORKSPACE_ID.fetch_add(1, Ordering::Relaxed);
    format!("w{}", encode_public_number(counter as usize))
}

pub(crate) fn encode_public_number(mut value: usize) -> String {
    if value == 0 {
        return "0".to_string();
    }

    let mut encoded = Vec::new();
    while value > 0 {
        let digit = (value - 1) % PUBLIC_ID_ALPHABET.len();
        encoded.push(PUBLIC_ID_ALPHABET[digit] as char);
        value = (value - 1) / PUBLIC_ID_ALPHABET.len();
    }
    encoded.iter().rev().collect()
}

pub(crate) fn decode_public_number(value: &str) -> Option<usize> {
    let mut decoded = 0usize;
    for ch in value.chars() {
        let digit = PUBLIC_ID_ALPHABET
            .iter()
            .position(|candidate| *candidate as char == ch)?;
        decoded = decoded
            .checked_mul(PUBLIC_ID_ALPHABET.len())?
            .checked_add(digit + 1)?;
    }
    Some(decoded)
}

pub(crate) fn public_workspace_number(id: &str) -> Option<usize> {
    id.strip_prefix('w').and_then(decode_public_number)
}

pub(crate) fn public_pane_id_for_number(workspace_id: &str, pane_number: usize) -> String {
    format!("{workspace_id}:p{}", encode_public_number(pane_number))
}

pub(crate) fn public_tab_id_for_number(workspace_id: &str, tab_number: usize) -> String {
    format!("{workspace_id}:t{}", encode_public_number(tab_number))
}

pub(crate) fn reserve_workspace_ids(workspaces: &[Workspace]) {
    let Some(next) = workspaces
        .iter()
        .filter_map(|workspace| public_workspace_number(&workspace.id))
        .max()
        .and_then(|max| u64::try_from(max.checked_add(1)?).ok())
    else {
        return;
    };

    let mut current = NEXT_WORKSPACE_ID.load(Ordering::Relaxed);
    while current < next {
        match NEXT_WORKSPACE_ID.compare_exchange_weak(
            current,
            next,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}
