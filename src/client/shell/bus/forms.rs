use super::editor::Editor;
use crate::bus::{
    launch::{AddAgent, PathSuggestion, SetupNotice},
    model::*,
};

#[derive(Clone, Debug)]
pub(super) enum Form {
    Help {
        scroll: usize,
    },
    Room(Editor),
    Agent {
        name: Editor,
        provider: Provider,
        cwd: Editor,
        args: Box<Editor>,
        field: usize,
    },
    Files(Editor),
    Consent {
        input: AddAgent,
        notice: SetupNotice,
    },
}

impl Form {
    pub fn editor_mut(&mut self) -> Option<&mut Editor> {
        match self {
            Self::Room(e) | Self::Files(e) => Some(e),
            Self::Agent {
                name,
                cwd,
                args,
                field,
                ..
            } => match field {
                0 => Some(name),
                2 => Some(cwd),
                3 => Some(args),
                _ => None,
            },
            _ => None,
        }
    }
    pub fn path_query(&self) -> Option<(String, bool)> {
        match self {
            Self::Files(editor) => Some((editor.text.clone(), false)),
            Self::Agent { cwd, field: 2, .. } => Some((cwd.text.clone(), true)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Suggestions {
    pub query_id: u64,
    pub entries: Vec<PathSuggestion>,
    pub selected: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RenameTarget {
    Room(RoomId),
    Agent(AgentId),
}

#[derive(Clone, Debug)]
pub(super) struct Rename {
    pub target: RenameTarget,
    pub editor: Editor,
}
