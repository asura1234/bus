use super::{fallback_label_from_cwd, Workspace};
use crate::{
    terminal::{TerminalRuntimeRegistry, TerminalState},
    utils::ids::TerminalId,
};
use std::collections::HashMap;
use std::path::PathBuf;

/// Automatic workspace label for `cwd`: the repository-relative name inside a Git checkout,
/// otherwise the directory name.
pub(crate) fn workspace_auto_label(cwd: &std::path::Path) -> String {
    super::git_label::git_space_metadata(cwd)
        .map(|space| super::git_label::automatic_workspace_label(cwd, &space.repo_root))
        .unwrap_or_else(|| fallback_label_from_cwd(cwd))
}

impl Workspace {
    pub fn set_custom_name(&mut self, name: String) {
        self.custom_name = Some(name);
    }

    pub fn resolved_identity_cwd_from(
        &self,
        terminals: &HashMap<TerminalId, TerminalState>,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) -> Option<PathBuf> {
        self.tabs
            .first()
            .and_then(|tab| tab.cwd_for_pane(tab.root_pane, terminals, terminal_runtimes))
            .or_else(|| Some(self.identity_cwd.clone()))
    }

    #[cfg(test)]
    pub fn display_name(&self) -> String {
        if let Some(name) = &self.custom_name {
            return name.clone();
        }

        self.automatic_display_name_for_cwd(&self.identity_cwd)
    }

    pub(crate) fn display_name_from_terminals(
        &self,
        terminals: &HashMap<TerminalId, TerminalState>,
    ) -> String {
        if let Some(name) = &self.custom_name {
            return name.clone();
        }

        let cwd = self
            .tabs
            .first()
            .and_then(|tab| tab.terminal_id(tab.root_pane))
            .and_then(|terminal_id| terminals.get(terminal_id))
            .map(|terminal| &terminal.cwd)
            .unwrap_or(&self.identity_cwd);
        self.automatic_display_name_for_cwd(cwd)
    }

    pub fn display_name_from(
        &self,
        terminals: &HashMap<TerminalId, TerminalState>,
        terminal_runtimes: &TerminalRuntimeRegistry,
    ) -> String {
        if let Some(name) = &self.custom_name {
            return name.clone();
        }

        self.automatic_display_name_for_cwd(
            &self
                .resolved_identity_cwd_from(terminals, terminal_runtimes)
                .expect("workspace always has an identity cwd"),
        )
    }

    fn automatic_display_name_for_cwd(&self, cwd: &std::path::Path) -> String {
        if cwd == self.cached_identity_cwd {
            self.cached_auto_label.clone()
        } else {
            fallback_label_from_cwd(cwd)
        }
    }
}
