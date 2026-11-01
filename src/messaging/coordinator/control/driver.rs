//! The production caller of the shared terminal driver. Selection reads each
//! redraw before another move; ordinary text and keys are sent exactly once.
use super::{
    optional_bool, optional_text, required, schema, Duration, Method, ResponseResult, Worker,
};
use crate::messaging::native::TransportError;
use serde_json::{json, Value};

const REDRAW_POLLS: usize = 20;
const REDRAW_INTERVAL: Duration = Duration::from_millis(100);
// Leave room for a native RPC to settle before the control reply's 15s deadline.
const SELECTION_BUDGET: Duration = Duration::from_secs(8);

impl Worker {
    pub(super) fn execute_driver(&mut self, method: &str, p: &Value) -> Result<Value, String> {
        let pane = optional_bool(p, "pane")?;
        let (target, managed_terminal) = if pane {
            (required(p, "agent")?.to_owned(), None)
        } else {
            let agent = self.control_agent(p)?;
            let (_, _, terminal, pane, _) = self.dialog_identity(agent)?;
            (pane, Some(terminal))
        };
        if method == "agent.observe" {
            let cells = optional_bool(p, "cells")?;
            let observation =
                native_observe(&target, managed_terminal.as_deref(), None, &mut |method| {
                    self.transport.request(method)
                })
                .map_err(|error| error.message)?;
            return serde_json::to_value(observation)
                .map(|observation| observation_json(observation, cells))
                .map_err(|error| error.to_string());
        }
        let explicit_terminal = optional_text(p, "terminal_id")?;
        if explicit_terminal.is_some_and(|expected| {
            managed_terminal
                .as_deref()
                .is_some_and(|actual| actual != expected)
        }) {
            return Err("Agent terminal identity changed; observe it again".into());
        }
        let text = p
            .get("text")
            .map(|value| value.as_str().map(str::to_owned).ok_or("Invalid text"))
            .transpose()?;
        let keys = p
            .get("keys")
            .map(|value| {
                serde_json::from_value::<Vec<String>>(value.clone()).map_err(|_| "Invalid keys")
            })
            .transpose()?
            .unwrap_or_default();
        let params = schema::AgentPressParams {
            target,
            expected_terminal_id: explicit_terminal.map(str::to_owned).or(managed_terminal),
            select: optional_text(p, "select")?.map(str::to_owned),
            text,
            keys,
            submit: optional_bool(p, "submit")?,
        };
        press_terminal(
            params,
            &mut |method| self.transport.request(method),
            &mut || std::thread::sleep(REDRAW_INTERVAL),
        )
    }
}

fn focus_identity(observation: &schema::TerminalObservation) -> Result<(String, usize), String> {
    observation
        .focus
        .as_ref()
        .map(|(_, label)| (label.clone(), observation.focus_occurrence))
        .ok_or_else(|| "Menu focus is not unique".into())
}

fn driver_error(message: &str) -> TransportError {
    TransportError {
        code: None,
        message: message.into(),
        definitely_rejected: false,
    }
}

fn native_observe(
    target: &str,
    expected_terminal: Option<&str>,
    selection_label: Option<&str>,
    call: &mut impl FnMut(Method) -> Result<ResponseResult, TransportError>,
) -> Result<schema::TerminalObservation, TransportError> {
    match call(Method::AgentObserve(schema::AgentObserveParams {
        target: target.into(),
        selection_label: selection_label.map(str::to_owned),
    }))? {
        ResponseResult::TerminalObserved { observation } => {
            if expected_terminal.is_some_and(|expected| expected != observation.terminal_id) {
                return Err(driver_error("Terminal identity changed; observe it again"));
            }
            Ok(observation)
        }
        _ => Err(driver_error("Unexpected terminal observation response")),
    }
}

fn press_terminal(
    params: schema::AgentPressParams,
    call: &mut impl FnMut(Method) -> Result<ResponseResult, TransportError>,
    pause: &mut impl FnMut(),
) -> Result<Value, String> {
    let started = std::time::Instant::now();
    press_terminal_with_elapsed(params, call, pause, &mut || started.elapsed())
}

fn press_terminal_with_elapsed(
    mut params: schema::AgentPressParams,
    call: &mut impl FnMut(Method) -> Result<ResponseResult, TransportError>,
    pause: &mut impl FnMut(),
    elapsed: &mut impl FnMut() -> Duration,
) -> Result<Value, String> {
    let mut keys = Vec::new();
    let mut text_written = false;
    let mut previous = None;
    loop {
        if let Some(press) = previous.take_if(|_| elapsed() >= SELECTION_BUDGET) {
            return selection_pending_result(press, keys, text_written);
        }
        let response = call(Method::AgentPress(params.clone()));
        let mut press = match response {
            Ok(ResponseResult::TerminalPressed { press }) => press,
            other => {
                let error = match other {
                    Err(error) => error.message,
                    Ok(_) => "Unexpected terminal input response; outcome is uncertain".into(),
                };
                return match previous {
                    Some(press) => press_result(press, keys, text_written, "unknown", Some(error)),
                    None => Err(error),
                };
            }
        };
        keys.extend(press.keys.iter().cloned());
        text_written |= press.text_written;
        if !press.selection_pending {
            let outcome = if params.select.is_some() {
                "selected"
            } else {
                "sent"
            };
            return press_result(press, keys, text_written, outcome, None);
        }
        // Bind even an explicitly selected plain pane after its first native
        // response, before following any redraw or continuing a selection.
        params.expected_terminal_id = Some(press.observation.terminal_id.clone());
        let focus = match focus_identity(&press.observation) {
            Ok(focus) => focus,
            Err(error) => {
                return press_result(press, keys, text_written, "selection_pending", Some(error))
            }
        };
        let mut moved = false;
        for _ in 0..REDRAW_POLLS {
            if elapsed() >= SELECTION_BUDGET {
                return selection_pending_result(press, keys, text_written);
            }
            pause();
            match native_observe(
                &params.target,
                params.expected_terminal_id.as_deref(),
                params.select.as_deref(),
                call,
            ) {
                Ok(observation) => {
                    let next_focus = focus_identity(&observation);
                    press.observation = observation;
                    match next_focus {
                        Ok(next_focus) if next_focus != focus => {
                            moved = true;
                            break;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            return press_result(
                                press,
                                keys,
                                text_written,
                                "selection_pending",
                                Some(error),
                            )
                        }
                    }
                }
                Err(error) if error.code.as_deref() == Some("terminal_observation_unavailable") => {
                }
                Err(error) => {
                    return press_result(press, keys, text_written, "unknown", Some(error.message))
                }
            }
        }
        if !moved {
            return press_result(
                press,
                keys,
                text_written,
                "selection_pending",
                Some(
                    "Menu focus did not change after navigation; observe before continuing".into(),
                ),
            );
        }
        previous = Some(press);
    }
}

fn selection_pending_result(
    press: schema::TerminalPressResult,
    keys: Vec<String>,
    text_written: bool,
) -> Result<Value, String> {
    press_result(
        press,
        keys,
        text_written,
        "selection_pending",
        Some("Selection is still pending; continue from the returned screen".into()),
    )
}

fn press_result(
    mut press: schema::TerminalPressResult,
    keys: Vec<String>,
    text_written: bool,
    outcome: &str,
    error: Option<String>,
) -> Result<Value, String> {
    press.keys = keys;
    press.text_written = text_written;
    let input_mode = if press.selection_pending || outcome == "selected" {
        "select"
    } else if text_written {
        "text"
    } else {
        "keys"
    };
    tracing::info!(event = "bus.terminal.driver.result",
        terminal_id = %press.observation.terminal_id, pane_id = %press.observation.pane_id,
        input_mode, outcome, reason = ?error, key_count = press.keys.len(), text_written,
        "Terminal driver input result");
    let mut result = serde_json::to_value(press).map_err(|error| error.to_string())?;
    result["observation"] = observation_json(result["observation"].take(), false);
    result["outcome"] = json!(outcome);
    if let Some(error) = error {
        result["reason"] = json!(error);
    }
    Ok(result)
}

/// Keep the ordinary tool response compact. The native API and selection loop
/// retain the full typed grid; only the returned control JSON is reduced.
fn observation_json(mut observation: Value, cells: bool) -> Value {
    observation["cols"] = observation["grid"]["cols"].clone();
    observation["rows"] = observation["grid"]["rows"].clone();
    observation["cursor"] = observation["grid"]["cursor"].clone();
    if !cells {
        if let Some(fields) = observation.as_object_mut() {
            fields.remove("grid");
        }
    }
    observation
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::api::schema::driver::Grid;
    use std::collections::VecDeque;

    fn observation(selected: usize, revision: u64) -> schema::TerminalObservation {
        let mut grid = Grid::blank(40, 4);
        for (row, label) in ["Install update now", "Continue", "Remind me later"]
            .iter()
            .enumerate()
        {
            let text = format!("{} {}", if row == selected { "❯" } else { " " }, label);
            for (col, ch) in text.chars().enumerate() {
                grid.cells[row][col].symbol = ch.to_string();
            }
        }
        schema::TerminalObservation {
            terminal_id: "term_1".into(),
            pane_id: "w1:p2".into(),
            content_revision: revision,
            text: grid.text(),
            focus: Some((
                selected,
                ["Install update now", "Continue", "Remind me later"][selected].into(),
            )),
            grid,
            focus_occurrence: 0,
        }
    }

    fn pressed(selected: usize, pending: bool, key: &str) -> ResponseResult {
        ResponseResult::TerminalPressed {
            press: schema::TerminalPressResult {
                keys: vec![key.into()],
                text_written: false,
                selection_pending: pending,
                observation: observation(selected, 2),
            },
        }
    }

    fn params(select: Option<&str>) -> schema::AgentPressParams {
        schema::AgentPressParams {
            target: "w1:p2".into(),
            expected_terminal_id: Some("term_1".into()),
            select: select.map(str::to_owned),
            text: None,
            keys: Vec::new(),
            submit: false,
        }
    }

    #[test]
    fn selecting_waits_for_focus_not_spinner_or_revision_before_the_next_key() {
        let mut script = VecDeque::from([
            pressed(0, true, "down"),
            ResponseResult::TerminalObserved {
                observation: observation(0, 4),
            },
            ResponseResult::TerminalObserved {
                observation: observation(1, 6),
            },
            pressed(1, true, "down"),
            ResponseResult::TerminalObserved {
                observation: observation(2, 8),
            },
            pressed(2, false, "enter"),
        ]);
        let mut methods = Vec::new();
        let result = press_terminal(
            params(Some("Remind me later")),
            &mut |method| {
                methods.push(method);
                Ok(script.pop_front().unwrap())
            },
            &mut || {},
        )
        .unwrap();
        assert!(script.is_empty());
        for method in &methods {
            if let Method::AgentObserve(observe) = method {
                assert_eq!(observe.selection_label.as_deref(), Some("Remind me later"));
            }
        }
        assert!(matches!(
            &methods[..],
            [
                Method::AgentPress(_),
                Method::AgentObserve(_),
                Method::AgentObserve(_),
                Method::AgentPress(_),
                Method::AgentObserve(_),
                Method::AgentPress(_)
            ]
        ));
        assert_eq!(result["keys"], json!(["down", "down", "enter"]));
        assert_eq!(result["outcome"], "selected");
        assert_eq!(result["selection_pending"], false);
    }

    #[test]
    fn a_menu_whose_focus_did_not_move_receives_one_navigation_key_and_returns_its_screen() {
        let mut writes = 0;
        let mut reads = 0;
        let result = press_terminal(
            params(Some("Continue")),
            &mut |method| match method {
                Method::AgentPress(_) => {
                    writes += 1;
                    Ok(pressed(0, true, "down"))
                }
                Method::AgentObserve(_) => {
                    reads += 1;
                    Ok(ResponseResult::TerminalObserved {
                        observation: observation(0, reads * 2),
                    })
                }
                _ => unreachable!(),
            },
            &mut || {},
        )
        .unwrap();
        assert_eq!(writes, 1);
        assert_eq!(reads, REDRAW_POLLS as u64);
        assert_eq!(result["keys"], json!(["down"]));
        assert_eq!(result["outcome"], "selection_pending");
        assert!(result["observation"]["text"]
            .as_str()
            .unwrap()
            .contains("Install update now"));
    }

    #[test]
    fn terminal_replacement_after_navigation_preserves_sent_evidence_without_confirming() {
        let mut calls = 0;
        let result = press_terminal(
            params(Some("Continue")),
            &mut |method| {
                calls += 1;
                match method {
                    Method::AgentPress(_) => Ok(pressed(0, true, "down")),
                    Method::AgentObserve(_) => {
                        let mut replaced = observation(1, 4);
                        replaced.terminal_id = "term_replacement".into();
                        Ok(ResponseResult::TerminalObserved {
                            observation: replaced,
                        })
                    }
                    _ => unreachable!(),
                }
            },
            &mut || {},
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result["keys"], json!(["down"]));
        assert_eq!(result["outcome"], "unknown");
        assert!(result["reason"]
            .as_str()
            .unwrap()
            .contains("identity changed"));
    }

    #[test]
    fn text_and_explicit_control_keys_are_forwarded_once_without_adding_submit() {
        let mut request = params(None);
        request.text = Some("  typed answer  ".into());
        request.keys = vec!["ctrl+c".into(), "2".into()];
        let mut calls = 0;
        let result = press_terminal(
            request.clone(),
            &mut |method| {
                calls += 1;
                let Method::AgentPress(actual) = method else {
                    panic!("unexpected read")
                };
                assert_eq!(actual, request);
                let ResponseResult::TerminalPressed { mut press } = pressed(0, false, "ctrl+c")
                else {
                    unreachable!()
                };
                press.keys.push("2".into());
                press.text_written = true;
                Ok(ResponseResult::TerminalPressed { press })
            },
            &mut || {},
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(result["text_written"], true);
        assert_eq!(result["keys"], json!(["ctrl+c", "2"]));
        assert_eq!(result["outcome"], "sent");
        assert!(result["observation"].get("grid").is_none());
        assert_eq!(result["observation"]["cols"], 40);
        assert_eq!(result["observation"]["rows"], 4);
    }

    #[test]
    fn control_observation_includes_styled_cells_only_when_requested() {
        let native = serde_json::to_value(observation(1, 4)).unwrap();
        let compact = observation_json(native.clone(), false);
        assert!(compact.get("grid").is_none());
        assert_eq!(compact["text"], native["text"]);
        assert_eq!(compact["focus"], native["focus"]);
        assert_eq!(compact["terminal_id"], "term_1");
        assert_eq!(compact["cols"], 40);
        assert_eq!(compact["rows"], 4);
        let styled = observation_json(native.clone(), true);
        assert_eq!(styled["grid"], native["grid"]);
    }

    #[test]
    fn relocating_the_focused_row_does_not_send_a_second_navigation_key() {
        let mut relocated = observation(0, 4);
        relocated
            .grid
            .cells
            .insert(0, relocated.grid.cells[3].clone());
        relocated.grid.rows += 1;
        relocated.focus = Some((1, "Install update now".into()));
        relocated.text = relocated.grid.text();
        let mut script = VecDeque::from([
            pressed(0, true, "down"),
            ResponseResult::TerminalObserved {
                observation: relocated,
            },
            ResponseResult::TerminalObserved {
                observation: observation(1, 6),
            },
            pressed(1, false, "enter"),
        ]);
        let result = press_terminal(
            params(Some("Continue")),
            &mut |_| Ok(script.pop_front().unwrap()),
            &mut || {},
        )
        .unwrap();
        assert_eq!(result["keys"], json!(["down", "enter"]));
        assert_eq!(result["outcome"], "selected");
        assert!(script.is_empty());
    }

    #[test]
    fn a_slow_selection_returns_its_receipt_before_the_control_deadline() {
        let elapsed = std::cell::Cell::new(Duration::ZERO);
        let mut script = VecDeque::from([
            pressed(0, true, "down"),
            ResponseResult::TerminalObserved {
                observation: observation(0, 4),
            },
            ResponseResult::TerminalObserved {
                observation: observation(1, 6),
            },
            pressed(1, true, "down"),
            ResponseResult::TerminalObserved {
                observation: observation(1, 8),
            },
        ]);
        let result = press_terminal_with_elapsed(
            params(Some("Remind me later")),
            &mut |_| Ok(script.pop_front().unwrap()),
            &mut || elapsed.set(elapsed.get() + Duration::from_secs(3)),
            &mut || elapsed.get(),
        )
        .unwrap();
        assert_eq!(result["keys"], json!(["down", "down"]));
        assert_eq!(result["outcome"], "selection_pending");
        assert_eq!(result["observation"]["focus"][1], "Continue");
        assert!(elapsed.get() < Duration::from_secs(15));
        assert!(script.is_empty());
        let resumed = press_terminal(
            params(Some("Remind me later")),
            &mut |_| Ok(pressed(2, false, "enter")),
            &mut || {},
        )
        .unwrap();
        assert_eq!(resumed["keys"], json!(["enter"]));
        assert_eq!(resumed["outcome"], "selected");
    }
    #[test]
    fn selecting_traverses_equal_intermediate_option_labels() {
        fn duplicate(selected: usize) -> schema::TerminalObservation {
            let mut result = observation(selected, 2);
            for (row, label) in ["Continue", "Continue", "Done"].iter().enumerate() {
                for cell in &mut result.grid.cells[row] {
                    cell.symbol = " ".into();
                }
                for (col, ch) in format!("{}{}", if row == selected { "❯" } else { " " }, label)
                    .chars()
                    .enumerate()
                {
                    result.grid.cells[row][col].symbol = ch.to_string();
                }
            }
            result.focus = Some((
                selected,
                if selected == 2 { "Done" } else { "Continue" }.into(),
            ));
            result.focus_occurrence = usize::from(selected == 1);
            result.text = result.grid.text();
            result
        }
        let mut script = VecDeque::from([
            ResponseResult::TerminalPressed {
                press: schema::TerminalPressResult {
                    keys: vec!["down".into()],
                    text_written: false,
                    selection_pending: true,
                    observation: duplicate(0),
                },
            },
            ResponseResult::TerminalObserved {
                observation: duplicate(1),
            },
            ResponseResult::TerminalPressed {
                press: schema::TerminalPressResult {
                    keys: vec!["down".into()],
                    text_written: false,
                    selection_pending: true,
                    observation: duplicate(1),
                },
            },
            ResponseResult::TerminalObserved {
                observation: duplicate(2),
            },
            ResponseResult::TerminalPressed {
                press: schema::TerminalPressResult {
                    keys: vec!["enter".into()],
                    text_written: false,
                    selection_pending: false,
                    observation: duplicate(2),
                },
            },
        ]);
        let result = press_terminal(
            params(Some("Done")),
            &mut |_| {
                Ok(script
                    .pop_front()
                    .expect("selection stalled on equal label"))
            },
            &mut || {},
        )
        .unwrap();
        assert!(script.is_empty());
        assert_eq!(result["outcome"], "selected");
        assert_eq!(result["keys"], json!(["down", "down", "enter"]));
    }

    #[test]
    fn selection_waits_through_transient_observation_unavailability() {
        let mut script = VecDeque::from([
            Ok(pressed(0, true, "down")),
            Err(TransportError {
                code: Some("terminal_observation_unavailable".into()),
                message: "Terminal is redrawing".into(),
                definitely_rejected: false,
            }),
            Ok(ResponseResult::TerminalObserved {
                observation: observation(1, 4),
            }),
            Ok(pressed(1, false, "enter")),
        ]);
        let result = press_terminal(
            params(Some("Continue")),
            &mut |_| script.pop_front().unwrap(),
            &mut || {},
        )
        .unwrap();
        assert!(script.is_empty());
        assert_eq!(result["outcome"], "selected");
        assert_eq!(result["keys"], json!(["down", "enter"]));
    }

    #[test]
    fn selection_does_not_retry_uncertain_transport_errors_or_failed_writes() {
        for failed_after_navigation in [false, true] {
            let mut calls = 0;
            let result = press_terminal(
                params(Some("Continue")),
                &mut |_| {
                    calls += 1;
                    if failed_after_navigation && calls == 1 {
                        Ok(pressed(0, true, "down"))
                    } else {
                        Err(driver_error("connection lost"))
                    }
                },
                &mut || {},
            );
            if failed_after_navigation {
                let receipt = result.unwrap();
                assert_eq!(receipt["outcome"], "unknown");
                assert_eq!(receipt["keys"], json!(["down"]));
                assert_eq!(calls, 2);
            } else {
                assert!(result.unwrap_err().contains("connection lost"));
                assert_eq!(calls, 1);
            }
        }
    }
}
