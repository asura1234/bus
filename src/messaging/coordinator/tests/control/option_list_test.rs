//! Claude Code option lists with a `Type something.` row, from real captures.
use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Native {
    screen: &'static str,
    /// Frames the agent draws after a write, one per observation.
    after_write: Vec<&'static str>,
    /// The first answer only focuses a hidden free-text row and shows this.
    focus_shows: Option<&'static str>,
    choices: Vec<schema::AgentDialogChooseParams>,
    answers: Vec<schema::AgentDialogAnswerParams>,
}

struct ScreenTransport(Arc<Mutex<Native>>);

fn observation(screen: &str) -> schema::AgentDialogObservation {
    let dialog = crate::agents::dialog::parse(screen).map(|dialog| schema::AgentDialog {
        kind: match dialog.kind {
            crate::agents::dialog::DialogKind::Choice => schema::AgentDialogKind::Choice,
            crate::agents::dialog::DialogKind::Question => schema::AgentDialogKind::Question,
        },
        id: dialog.id(),
        digest: dialog.digest(),
        free_text: dialog.free_text_summary().map(|(option, draft, checkbox)| {
            schema::AgentDialogFreeText {
                option,
                draft,
                checkbox,
            }
        }),
        tabs: dialog
            .tabs
            .into_iter()
            .map(|tab| schema::AgentDialogTab {
                label: tab.label,
                answered: tab.answered,
                active: tab.active,
            })
            .collect(),
        text: dialog.text,
        hint: dialog.hint,
        options: dialog
            .options
            .into_iter()
            .map(|option| schema::AgentDialogOption {
                number: option.number,
                label: option.label,
                selected: option.selected,
                free_text: option.free_text,
            })
            .collect(),
    });
    schema::AgentDialogObservation {
        pending_question: false,
        terminal_id: "term_internal".into(),
        pane_id: "w1:p2".into(),
        session_id: None,
        content_revision: 7,
        dialog,
    }
}

impl Transport for ScreenTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let mut native = self.0.lock().unwrap();
        let written = |native: &mut Native, keys: Vec<String>| {
            native.screen = "";
            Ok(ResponseResult::AgentDialogChosen {
                choice: schema::AgentDialogChooseResult {
                    written: true,
                    reason: None,
                    keys,
                    observation: observation(""),
                },
            })
        };
        match method {
            Method::AgentDialogObserve(_) => {
                if !native.after_write.is_empty() {
                    native.screen = native.after_write.remove(0);
                }
                Ok(ResponseResult::AgentDialog {
                    observation: observation(native.screen),
                })
            }
            Method::AgentDialogChoose(params) => {
                native.choices.push(params);
                written(&mut native, vec!["enter".into()])
            }
            Method::AgentDialogAnswer(params) => {
                native.answers.push(params);
                if let Some(focused) = native.focus_shows.take() {
                    native.screen = focused;
                    return Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written: true,
                            reason: Some("free_text_focused".into()),
                            keys: vec!["down".into(); 4],
                            observation: observation(focused),
                        },
                    });
                }
                written(&mut native, vec!["down".into(), "enter".into()])
            }
            other => panic!("unexpected native method: {other:?}"),
        }
    }
}

fn option_list_worker(screen: &'static str) -> (Worker, AgentId, PathBuf, Arc<Mutex<Native>>) {
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("term_internal".into()),
                pane_id: Some("w1:p2".into()),
                session_id: None,
            },
        )
        .unwrap();
    let native = Arc::new(Mutex::new(Native {
        screen,
        ..Native::default()
    }));
    worker.transport = Box::new(ScreenTransport(native.clone()));
    (worker, agent, dir, native)
}

fn fingerprint(worker: &mut Worker, agent: AgentId) -> (Value, String) {
    let observed = worker.observe_dialog(agent).unwrap();
    let fingerprint = observed["fingerprint"].as_str().unwrap().to_owned();
    (observed, fingerprint)
}

#[test]
fn free_text_on_an_option_list_is_one_answer_and_never_a_bare_choice() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-option-list/tall-rest.ansi"
    ));
    let (observed, fingerprint) = fingerprint(&mut worker, agent);
    assert_eq!(observed["dialog"]["kind"], "choice");
    assert_eq!(observed["dialog"]["options"][4]["free_text"], true);
    assert_eq!(observed["dialog"]["options"][0].get("free_text"), None);

    // Enter on the empty field declines the question, so choose refuses it
    // before spending the fingerprint.
    let refused = worker
        .choose_dialog_option(agent, 5, &fingerprint)
        .unwrap_err();
    assert!(refused.contains("agent answer"), "{refused}");
    assert!(native.lock().unwrap().choices.is_empty());
    assert!(worker
        .answer_dialog(agent, None, true, &fingerprint)
        .unwrap_err()
        .contains("agent choose"));

    let answered = worker
        .answer_dialog(agent, Some("Remap whole clips only"), false, &fingerprint)
        .unwrap();
    assert_eq!(answered["outcome"], "closed");
    let answers = &native.lock().unwrap().answers;
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].text.as_deref(), Some("Remap whole clips only"));
    assert!(!answers[0].skip);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_scrolled_option_list_offers_its_hidden_numbers_and_refuses_its_hidden_free_text_row() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-option-list/short-cursor4-scrolled.ansi"
    ));
    let (observed, fingerprint) = fingerprint(&mut worker, agent);
    let numbers: Vec<_> = observed["dialog"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|option| option["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [2, 3, 4, 6]);
    assert_eq!(observed["dialog"]["free_text"]["option"], 5);
    assert!(worker
        .choose_dialog_option(agent, 5, &fingerprint)
        .unwrap_err()
        .contains("agent answer"));
    let chosen = worker.choose_dialog_option(agent, 6, &fingerprint).unwrap();
    assert_eq!(chosen["outcome"], "closed");
    assert_eq!(native.lock().unwrap().choices[0].option, 6);
    assert!(native.lock().unwrap().answers.is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_tabbed_form_reports_its_tabs_and_answers_its_free_text_row_with_text() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-option-list/tabs-q1-rest.ansi"
    ));
    let (observed, fingerprint) = fingerprint(&mut worker, agent);
    assert_eq!(
        observed["dialog"]["tabs"],
        json!([
            {"label": "Fresh files", "answered": false, "active": true},
            {"label": "Helper home", "answered": false, "active": false},
            {"label": "Submit", "answered": false, "active": false},
        ])
    );
    assert_eq!(observed["dialog"]["options"][2]["free_text"], true);
    assert!(worker
        .choose_dialog_option(agent, 3, &fingerprint)
        .unwrap_err()
        .contains("agent answer"));
    let answered = worker
        .answer_dialog(agent, Some("Fresh dir per run"), false, &fingerprint)
        .unwrap();
    assert_eq!(answered["outcome"], "closed");
    assert_eq!(
        native.lock().unwrap().answers[0].text.as_deref(),
        Some("Fresh dir per run")
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_filled_field_before_claude_takes_enter_is_not_reported_as_a_new_dialog() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-option-list/tall-rest.ansi"
    ));
    let (_, fingerprint) = fingerprint(&mut worker, agent);
    // Arrows and text land before the delayed Enter: the focused field shows
    // the answer for a moment, then Claude takes it and the list closes.
    let filled = include_str!(
        "../../../../../tests/fixtures/claude-option-list/tall-freetext-with-text-focused.ansi"
    );
    native.lock().unwrap().after_write = vec![filled, filled, ""];
    let answered = worker
        .answer_dialog(
            agent,
            Some("Combined write: arrows then paste"),
            false,
            &fingerprint,
        )
        .unwrap();
    assert_eq!(answered["outcome"], "closed");
    assert_eq!(answered["dialog"], Value::Null);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_short_pane_at_rest_can_choose_an_option_scrolled_below_the_window() {
    let dialog = crate::agents::dialog::parse(include_str!(
        "../../../../../tests/fixtures/claude-option-list/short-rest-top.ansi"
    ))
    .unwrap();
    // Claude scrolls with the selection, as it does on the way to the visible 6.
    assert_eq!(
        dialog.keys_for(6),
        Some(vec!["down", "down", "down", "down", "down", "enter"])
    );
    assert_eq!(
        dialog.keys_for(4),
        Some(vec!["down", "down", "down", "enter"]),
        "option 4 exists between the visible 3 and 6"
    );
}

#[test]
fn a_short_pane_at_rest_takes_a_free_text_answer() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-option-list/short-rest-top.ansi"
    ));
    let (_, fingerprint) = fingerprint(&mut worker, agent);
    let answered = worker.answer_dialog(agent, Some("Remap whole clips only"), false, &fingerprint);
    assert!(answered.is_ok(), "{answered:?}");
    assert_eq!(native.lock().unwrap().answers.len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Claude 2.1.308 after Enter in a multi-select form's `Type something`
/// checkbox (capture `multi-q1-enter-empty`, here holding the typed text):
/// the box is ticked and the field keeps focus.
const MULTI_SELECT_FIELD_TICKED_AND_FOCUSED: &str = "←  ☐ Caches  ☐ Region  ✔ Submit  →\n\nWhich caches may be removed?\n\n  1. [ ] Build cache\n         Remove the build cache.\n  2. [ ] Package cache\n         Remove the package cache.\n❯ 3. [✔] Keep only the newest cache\n     Next\n────────────────────────────────────────\n  4. Chat about this\n\nEnter to select · Tab/Arrow keys to navigate · ctrl+g to edit in Vim · Esc to cancel\n";

#[test]
fn a_multi_select_free_text_answer_leaves_a_way_on_to_next() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-question/multi-select.txt"
    ));
    let (observed, first) = fingerprint(&mut worker, agent);
    assert_eq!(observed["dialog"]["free_text"]["checkbox"], true);
    native.lock().unwrap().after_write = vec![MULTI_SELECT_FIELD_TICKED_AND_FOCUSED; 64];
    worker
        .answer_dialog(agent, Some("Keep only the newest cache"), false, &first)
        .unwrap();
    let (after, again) = fingerprint(&mut worker, agent);
    // The form still waits for Next or Submit; the orchestrator must be able
    // to choose it without pressing Enter in the field again.
    assert_eq!(after["dialog"]["kind"], "choice", "{after}");
    assert!(worker.choose_dialog_option(agent, 4, &again).is_ok());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_short_pane_can_choose_an_option_scrolled_above_the_window() {
    let dialog = crate::agents::dialog::parse(include_str!(
        "../../../../../tests/fixtures/claude-option-list/short-cursor4-scrolled.ansi"
    ))
    .unwrap();
    assert_eq!(dialog.keys_for(2), Some(vec!["up", "up", "enter"]));
    assert_eq!(
        dialog.keys_for(1),
        Some(vec!["up", "up", "up", "enter"]),
        "option 1 exists above the visible 2"
    );
}

const SHORT_FIELD_FOCUSED: &str = include_str!(
    "../../../../../tests/fixtures/claude-option-list/short-freetext-focused-empty.ansi"
);

#[test]
fn a_hidden_free_text_row_is_focused_and_checked_empty_before_any_text() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-option-list/short-rest-top.ansi"
    ));
    let (_, fingerprint) = fingerprint(&mut worker, agent);
    native.lock().unwrap().focus_shows = Some(SHORT_FIELD_FOCUSED);
    let answered = worker
        .answer_dialog(agent, Some("Remap whole clips only"), false, &fingerprint)
        .unwrap();
    assert_eq!(answered["outcome"], "closed");
    let answers = &native.lock().unwrap().answers;
    assert_eq!(
        answers.len(),
        2,
        "focus, then the text into the empty field"
    );
    let focused = crate::agents::dialog::parse(SHORT_FIELD_FOCUSED).unwrap();
    assert_eq!(answers[1].expected_dialog_digest, focused.digest());
    assert_eq!(answers[1].text.as_deref(), Some("Remap whole clips only"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_hidden_free_text_row_holding_a_draft_gets_no_text() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-option-list/short-rest-top.ansi"
    ));
    let (_, fingerprint) = fingerprint(&mut worker, agent);
    let drafted: &'static str = Box::leak(
        SHORT_FIELD_FOCUSED
            .replace("Type something.", "old draft")
            .into_boxed_str(),
    );
    native.lock().unwrap().focus_shows = Some(drafted);
    let refused = worker
        .answer_dialog(agent, Some("Remap whole clips only"), false, &fingerprint)
        .unwrap_err();
    assert!(refused.contains("free_text_holds_draft"), "{refused}");
    assert_eq!(native.lock().unwrap().answers.len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_draft_bus_moved_into_is_not_typed_over_by_the_next_answer() {
    let (mut worker, agent, dir, native) = option_list_worker(include_str!(
        "../../../../../tests/fixtures/claude-option-list/short-rest-top.ansi"
    ));
    let (_, first) = fingerprint(&mut worker, agent);
    let drafted: &'static str = Box::leak(
        SHORT_FIELD_FOCUSED
            .replace("Type something.", "old draft")
            .into_boxed_str(),
    );
    native.lock().unwrap().focus_shows = Some(drafted);
    assert!(worker
        .answer_dialog(agent, Some("Remap whole clips only"), false, &first)
        .unwrap_err()
        .contains("free_text_holds_draft"));
    // Bus left the cursor inside the draft at a place it cannot know; the
    // orchestrator's natural retry must not paste there and press Enter.
    let (observed, again) = fingerprint(&mut worker, agent);
    assert_eq!(observed["dialog"]["kind"], "question");
    assert_eq!(observed["dialog"]["free_text"]["draft"], true);
    let retried = worker.answer_dialog(agent, Some("Remap whole clips only"), false, &again);
    assert!(
        retried
            .as_ref()
            .is_err_and(|error| error.contains("free_text_holds_draft")),
        "{retried:?}"
    );
    assert_eq!(native.lock().unwrap().answers.len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_ticked_multi_select_field_offers_next_as_a_choice() {
    let dialog = crate::agents::dialog::parse(MULTI_SELECT_FIELD_TICKED_AND_FOCUSED).unwrap();
    let labels: Vec<_> = dialog
        .options
        .iter()
        .map(|option| (option.number, option.label.as_str(), option.free_text))
        .collect();
    let next = dialog
        .options
        .iter()
        .find(|option| option.label == "Next")
        .unwrap_or_else(|| panic!("no Next option: {labels:?}"));
    assert!(dialog.keys_for(next.number).is_some(), "{labels:?}");
}
