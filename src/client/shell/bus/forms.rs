use super::editor::Editor;
use crate::bus::{
    launch::{AddAgent, PathSuggestion, SetupNotice},
    model::*,
    orchestrator::{self, OrchestratorSpec, PromptValues},
};

#[derive(Clone, Debug)]
pub(super) enum Form {
    Help {
        scroll: usize,
    },
    Settings,
    Room(Editor),
    Agent {
        name: Editor,
        provider: Option<Provider>,
        provider_cursor: Provider,
        cwd: Editor,
        args: Box<Editor>,
        field: usize,
        /// Present only when adding a MASTER agent: the work room it will orchestrate.
        orchestrates: Option<Orchestrates>,
        /// Present only when adding a MASTER agent: its system prompt.
        prompt: Option<Box<PromptField>>,
    },
    Files(Editor),
    /// Reassigns or unassigns the room a MASTER agent orchestrates.
    Orchestrate {
        agent: AgentId,
        choice: Orchestrates,
    },
    Consent {
        input: AddAgent,
        orchestrator: Option<OrchestratorSpec>,
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
                prompt,
                ..
            } => match *field {
                0 => Some(name),
                2 => Some(cwd),
                3 => Some(args),
                PROMPT_FIELD => prompt.as_mut().map(|prompt| &mut prompt.editor),
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

/// The MASTER agent form's choice; `None` adds an unassigned orchestrator.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Orchestrates(pub Option<RoomId>);

/// The field index of the MASTER-only Orchestrates choice.
pub(super) const ORCHESTRATES_FIELD: usize = 4;
/// The field index of the MASTER-only multi-line system prompt.
pub(super) const PROMPT_FIELD: usize = 5;

/// A MASTER agent's system prompt and the text Bus last filled in, so a
/// change of room or name re-fills it only until the human edits it.
#[derive(Clone, Debug)]
pub(super) struct PromptField {
    pub editor: Editor,
    pub filled: String,
}

impl Form {
    /// How many fields Tab cycles through in the agent form.
    pub fn agent_field_count(&self) -> usize {
        match self {
            Self::Agent {
                orchestrates: Some(_),
                ..
            } => PROMPT_FIELD + 1,
            _ => ORCHESTRATES_FIELD,
        }
    }

    /// Fills the default orchestrator prompt for the form's name and `room`,
    /// unless the human has edited the prompt since Bus last filled it.
    pub fn refill_prompt(&mut self, room: Option<(String, RoomId)>, docs: &std::path::Path) {
        let Self::Agent {
            name,
            prompt: Some(prompt),
            ..
        } = self
        else {
            return;
        };
        if prompt.editor.text != prompt.filled {
            return;
        }
        let text = orchestrator::fill(
            orchestrator::DEFAULT_PROMPT,
            &PromptValues {
                room,
                agent: name.text.trim().to_owned(),
                docs: docs.to_path_buf(),
            },
        );
        if text != prompt.filled {
            prompt.editor = Editor::new(text.clone());
            prompt.editor.cursor = 0;
            prompt.filled = text;
        }
    }
}

impl super::BusUi {
    /// Re-fills an open MASTER agent form's prompt for its current room and name.
    pub(super) fn refill_orchestrator_prompt(&mut self) {
        let Some(Form::Agent {
            orchestrates: Some(choice),
            ..
        }) = &self.form
        else {
            return;
        };
        let room = choice
            .0
            .and_then(|room| self.snapshot.state.room(room))
            .map(|room| (room.name.clone(), room.id));
        let docs = crate::bus::entry::data_dir().map_or_else(
            || std::path::PathBuf::from("<BUS_DATA_DIR>/docs"),
            |data| orchestrator::docs_dir(&data),
        );
        if let Some(form) = &mut self.form {
            form.refill_prompt(room, &docs);
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
