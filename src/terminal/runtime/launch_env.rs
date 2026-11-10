//! Per-pane environment and identity shared across shell launches.

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PaneLaunchEnv {
    pub(super) extra: Vec<(String, String)>,
    pub(super) identity: PaneLaunchIdentity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum PaneLaunchIdentity {
    #[default]
    Inherit,
    Managed {
        workspace_id: String,
        tab_id: String,
        pane_id: String,
    },
}

impl PaneLaunchEnv {
    pub(crate) fn extra(&self) -> &[(String, String)] {
        &self.extra
    }

    pub(crate) fn from_extra(extra: Vec<(String, String)>) -> Self {
        Self {
            extra,
            identity: PaneLaunchIdentity::Inherit,
        }
    }

    pub(crate) fn with_identity(
        mut self,
        workspace_id: String,
        tab_id: String,
        pane_id: String,
    ) -> Self {
        self.identity = PaneLaunchIdentity::Managed {
            workspace_id,
            tab_id,
            pane_id,
        };
        self
    }
}
