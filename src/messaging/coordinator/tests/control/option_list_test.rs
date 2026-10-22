//! Claude Code option lists with a `Type something.` row, from real captures.
use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Native {
    screen: &'static str,
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
            Method::AgentDialogObserve(_) => Ok(ResponseResult::AgentDialog {
                observation: observation(native.screen),
            }),
            Method::AgentDialogChoose(params) => {
                native.choices.push(params);
                written(&mut native, vec!["enter".into()])
            }
            Method::AgentDialogAnswer(params) => {
                native.answers.push(params);
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
fn a_scrolled_option_list_offers_its_hidden_numbers_but_no_hidden_free_text() {
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
    assert!(worker
        .answer_dialog(agent, Some("text"), false, &fingerprint)
        .unwrap_err()
        .contains("agent choose"));
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
