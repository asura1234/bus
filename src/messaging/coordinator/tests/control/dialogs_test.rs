use super::*;

#[test]
fn agent_dialog_choose_is_identity_bound_single_use_and_reports_the_outcome() {
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Native {
        shown: bool,
        written: bool,
        question: bool,
        choices: Vec<schema::AgentDialogChooseParams>,
        answers: Vec<schema::AgentDialogAnswerParams>,
    }
    struct DialogTransport(Arc<Mutex<Native>>);
    fn observation(shown: bool) -> schema::AgentDialogObservation {
        schema::AgentDialogObservation {
            pending_question: false,
            terminal_id: "term_internal".into(),
            pane_id: "w1:p2".into(),
            session_id: None,
            content_revision: 22,
            dialog: shown.then(|| schema::AgentDialog {
                kind: schema::AgentDialogKind::Choice,
                text: "Do you trust the contents of this directory?".into(),
                options: vec![
                    schema::AgentDialogOption {
                        number: 1,
                        label: "Yes, continue".into(),
                        selected: true,
                    },
                    schema::AgentDialogOption {
                        number: 2,
                        label: "No, quit".into(),
                        selected: false,
                    },
                ],
                hint: Some("Press enter to continue".into()),
                id: "dialog-id".into(),
                digest: "dialog-digest".into(),
            }),
        }
    }
    impl Transport for DialogTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let mut native = self.0.lock().unwrap();
            match method {
                Method::AgentDialogObserve(params) => {
                    assert_eq!(params.target, "w1:p2");
                    let mut observed = observation(native.shown);
                    if native.question {
                        if let Some(dialog) = &mut observed.dialog {
                            dialog.kind = schema::AgentDialogKind::Question;
                            dialog.options.clear();
                        }
                    }
                    Ok(ResponseResult::AgentDialog {
                        observation: observed,
                    })
                }
                Method::AgentDialogChoose(params) => {
                    native.choices.push(params);
                    let written = native.written;
                    native.shown = !written;
                    Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written,
                            reason: (!written).then(|| "stale_or_changed_dialog".into()),
                            keys: if written {
                                vec!["down".into(), "enter".into()]
                            } else {
                                vec![]
                            },
                            observation: observation(native.shown),
                        },
                    })
                }
                Method::AgentDialogAnswer(params) => {
                    let keys = if params.skip {
                        vec!["ctrl+]".into()]
                    } else {
                        vec!["enter".into()]
                    };
                    native.answers.push(params);
                    let written = native.written;
                    native.shown = !written;
                    Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written,
                            reason: (!written).then(|| "stale_or_changed_dialog".into()),
                            keys: if written { keys } else { vec![] },
                            observation: observation(native.shown),
                        },
                    })
                }
                other => panic!("unexpected native method: {other:?}"),
            }
        }
    }

    let (mut worker, _room, agent, dir) = fixture();
    // Still launching: the trust dialog comes before any session is bound.
    let identity = |launch: &str| AgentRuntimeIdentity {
        launch_id: Some(launch.into()),
        terminal_id: Some("term_internal".into()),
        pane_id: Some("w1:p2".into()),
        session_id: None,
    };
    worker
        .state
        .set_agent_runtime_identity(agent, identity("launch"))
        .unwrap();
    let relaunch = |worker: &mut Worker, launch: &str| {
        let mut state = worker.state.clone();
        state
            .set_agent_runtime_identity(agent, identity(launch))
            .unwrap();
        worker.save(state).unwrap();
    };
    let native = Arc::new(Mutex::new(Native::default()));
    worker.transport = Box::new(DialogTransport(native.clone()));

    let none = call(
        &mut worker,
        "observe-none",
        "agent.dialog.observe",
        json!({"agent":"codex1"}),
    );
    assert!(none.ok, "{none:?}");
    assert_eq!(none.result["dialog"], Value::Null);
    assert_eq!(none.result["fingerprint"], Value::Null);

    native.lock().unwrap().shown = true;
    let observed = call(
        &mut worker,
        "observe",
        "agent.dialog.observe",
        json!({"agent":"codex1"}),
    );
    assert_eq!(observed.result["dialog"]["options"][1]["label"], "No, quit");
    let fingerprint = observed.result["fingerprint"].as_str().unwrap().to_owned();
    // Mutations replay by request ID, so each attempt needs its own.
    let attempts = std::cell::Cell::new(0);
    let choose = |worker: &mut Worker, option: &str, fingerprint: &str| {
        attempts.set(attempts.get() + 1);
        call(
            worker,
            &format!("choose-{}", attempts.get()),
            "agent.dialog.choose",
            json!({"agent":"codex1","option":option,"fingerprint":fingerprint}),
        )
    };

    // A relaunched agent, a missing option or a forged fingerprint sends nothing.
    relaunch(&mut worker, "replacement-launch");
    assert!(!choose(&mut worker, "2", &fingerprint).ok);
    relaunch(&mut worker, "launch");
    assert!(!choose(&mut worker, "3", &fingerprint).ok);
    assert!(!choose(&mut worker, "1", &fingerprint.replace("d1.", "d1.x")).ok);
    assert!(native.lock().unwrap().choices.is_empty());

    // The native layer rejects a dialog that changed; the fingerprint is spent.
    let stale = choose(&mut worker, "2", &fingerprint);
    assert!(!stale.ok);
    let message = stale.error.unwrap().message;
    assert!(message.contains("No keys were sent"), "{message}");
    assert_eq!(native.lock().unwrap().choices.len(), 1);
    assert!(!choose(&mut worker, "2", &fingerprint).ok);
    assert_eq!(native.lock().unwrap().choices.len(), 1);

    let fresh = call(
        &mut worker,
        "observe-again",
        "agent.dialog.observe",
        json!({"agent":"codex1"}),
    );
    let fresh = fresh.result["fingerprint"].as_str().unwrap().to_owned();
    assert_ne!(fresh, fingerprint);
    native.lock().unwrap().written = true;
    let chosen = choose(&mut worker, "2", &fresh);
    assert!(chosen.ok, "{chosen:?}");
    assert_eq!(chosen.result["outcome"], "closed");
    assert_eq!(chosen.result["keys"], json!(["down", "enter"]));
    let sent = native.lock().unwrap().choices[1].clone();
    assert_eq!(sent.option, 2);
    assert_eq!(sent.expected_dialog_digest, "dialog-digest");
    assert_eq!(sent.expected_session_id, None);

    assert!(!choose(&mut worker, "2", &fresh).ok);
    assert_eq!(native.lock().unwrap().choices.len(), 2);
    let saved = worker.store.load().unwrap().unwrap();
    assert!(saved.dialog_fingerprint_consumed(&fresh));
    assert!(worker
        .answer_dialog(agent, Some("token"), false, &fresh)
        .is_err());
    native.lock().unwrap().question = true;
    native.lock().unwrap().shown = true;
    let fingerprint = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    for (text, skip) in [(None, false), (Some("token"), true), (Some(" "), false)] {
        assert!(worker
            .answer_dialog(agent, text, skip, &fingerprint)
            .is_err());
    }
    relaunch(&mut worker, "other-launch");
    assert!(worker
        .answer_dialog(agent, Some("token"), false, &fingerprint)
        .is_err());
    relaunch(&mut worker, "launch");
    assert!(native.lock().unwrap().answers.is_empty());
    native.lock().unwrap().written = false;
    assert!(worker
        .answer_dialog(agent, Some("token"), false, &fingerprint)
        .is_err());
    assert_eq!(native.lock().unwrap().answers.len(), 1);
    assert!(worker
        .answer_dialog(agent, Some("token"), false, &fingerprint)
        .is_err());
    assert_eq!(native.lock().unwrap().answers.len(), 1);
    native.lock().unwrap().written = true;
    let fresh = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let answered = worker
        .answer_dialog(agent, Some("token"), false, &fresh)
        .unwrap();
    assert_eq!(answered["outcome"], "closed");
    assert!(answered.get("option").is_none());
    assert!(worker.state.agent(agent).unwrap().dialog_answer.is_none());
    // A real newly reported question clears any previously recorded option.
    worker
        .state
        .set_dialog_notice(agent, Some("question".into()))
        .unwrap();
    native.lock().unwrap().shown = true;
    let fresh = worker.observe_dialog(agent).unwrap()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let skipped = worker.answer_dialog(agent, None, true, &fresh).unwrap();
    assert_eq!(skipped["outcome"], "closed");
    assert_eq!(skipped["keys"], json!(["ctrl+]"]));
    assert!(worker.state.agent(agent).unwrap().dialog_answer.is_none());
    assert!(worker.answer_dialog(agent, None, true, &fresh).is_err());
    assert!(worker
        .store
        .load()
        .unwrap()
        .unwrap()
        .dialog_fingerprint_consumed(&fresh));
    let answers = &native.lock().unwrap().answers;
    assert_eq!(answers[1].text.as_deref(), Some("token"));
    assert_eq!(answers[2].text, None);
    assert!(answers[2].skip);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_queued_question_redraw_returns_a_fingerprint_then_allows_choose_and_answer() {
    use std::sync::{Arc, Mutex};
    struct Native {
        stage: u32,
        opens: Vec<bool>,
    }
    struct QueuedTransport(Arc<Mutex<Native>>);
    fn observation(stage: u32) -> schema::AgentDialogObservation {
        let screen = match stage {
            1 => include_str!("../../../../../tests/fixtures/codex-question/expanded.txt"),
            2 => include_str!("../../../../../tests/fixtures/codex-question/other-selected.txt"),
            _ => "",
        };
        let dialog = crate::agents::dialog::parse(screen).map(|dialog| schema::AgentDialog {
            kind: match dialog.kind {
                crate::agents::dialog::DialogKind::Choice => schema::AgentDialogKind::Choice,
                crate::agents::dialog::DialogKind::Question => schema::AgentDialogKind::Question,
            },
            id: dialog.id(),
            digest: dialog.digest(),
            text: dialog.text,
            hint: dialog.hint,
            options: dialog
                .options
                .into_iter()
                .map(|option| schema::AgentDialogOption {
                    number: option.number,
                    label: option.label,
                    selected: option.selected,
                })
                .collect(),
        });
        schema::AgentDialogObservation {
            pending_question: stage == 0,
            terminal_id: "terminal".into(),
            pane_id: "pane".into(),
            session_id: None,
            content_revision: stage as u64,
            dialog,
        }
    }
    impl Transport for QueuedTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            let mut native = self.0.lock().unwrap();
            match method {
                Method::AgentDialogObserve(params) => {
                    native.opens.push(params.open_pending_question);
                    if native.stage == 0 && native.opens.len() == 3 {
                        native.stage = 1;
                    }
                    Ok(ResponseResult::AgentDialog {
                        observation: observation(native.stage),
                    })
                }
                Method::AgentDialogChoose(params) => {
                    assert_eq!(params.option, 3);
                    assert_eq!(
                        params.expected_dialog_digest,
                        observation(1).dialog.unwrap().digest
                    );
                    native.stage = 2;
                    Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written: true,
                            reason: None,
                            keys: vec!["down".into(), "down".into(), "enter".into()],
                            observation: observation(2),
                        },
                    })
                }
                Method::AgentDialogAnswer(params) => {
                    assert_eq!(params.text.as_deref(), Some("capture answer"));
                    assert_eq!(
                        params.expected_dialog_digest,
                        observation(2).dialog.unwrap().digest
                    );
                    native.stage = 3;
                    Ok(ResponseResult::AgentDialogChosen {
                        choice: schema::AgentDialogChooseResult {
                            written: true,
                            reason: None,
                            keys: vec!["enter".into()],
                            observation: observation(3),
                        },
                    })
                }
                other => panic!("unexpected request: {other:?}"),
            }
        }
    }
    let (mut worker, _, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: None,
            },
        )
        .unwrap();
    let native = Arc::new(Mutex::new(Native {
        stage: 0,
        opens: Vec::new(),
    }));
    worker.transport = Box::new(QueuedTransport(native.clone()));
    let observed = worker.observe_dialog(agent).unwrap();
    assert_eq!(
        native.lock().unwrap().opens,
        [true, false, false],
        "open only once, then poll without navigation"
    );
    assert_eq!(observed["dialog"]["options"][2]["label"], "Other");
    let fingerprint = observed["fingerprint"].as_str().unwrap();
    assert_eq!(
        worker.choose_dialog_option(agent, 3, fingerprint).unwrap()["outcome"],
        "replaced"
    );
    assert!(worker.choose_dialog_option(agent, 3, fingerprint).is_err());
    let other = worker.observe_dialog(agent).unwrap();
    assert_eq!(other["dialog"]["kind"], "question");
    let fingerprint = other["fingerprint"].as_str().unwrap();
    assert_eq!(
        worker
            .answer_dialog(agent, Some("capture answer"), false, fingerprint)
            .unwrap()["outcome"],
        "closed"
    );
    assert!(worker
        .answer_dialog(agent, Some("replay"), false, fingerprint)
        .is_err());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
