//! Shared reply display is a history projection; individual status remains lossless.
use super::*;

fn batch_history_fixture() -> (Fixture, Vec<RequestId>) {
    let mut f = Fixture::new();
    let requests: Vec<_> = ["first task", "second task", "third task"]
        .into_iter()
        .map(|text| f.send(Author::Developer, text))
        .collect();
    let mut state = f.worker.state.clone();
    assert!(state.coalesce_queue(requests[0]).is_some());
    for id in &requests {
        state.finish_for_test(*id, "SHARED_BATCH_ANSWER", io::now_ms());
    }
    f.worker.save(state).unwrap();
    (f, requests)
}

fn batch_history_assert_projection(f: &mut Fixture, requests: &[RequestId]) {
    let lead_message = f.worker.state.request(requests[0]).unwrap().prompt.id;
    let response = f.call("room.history", json!({"room":f.room.0.to_string()}));
    assert!(response.ok, "{response:?}");
    let messages = response.result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(message["delivery"]["complete"], true);
        let request = &message["delivery"]["requests"][0];
        assert_eq!(request["stage"], "replied");
        if index == 0 {
            assert_eq!(request["reply"]["text"], "SHARED_BATCH_ANSWER");
            assert!(request["answered_with_message"].is_null());
        } else {
            assert!(
                request["reply"].is_null(),
                "shared body repeated: {request}"
            );
            assert_eq!(request["answered_with_message"], json!(lead_message));
        }
    }
    // Deduplication is a history projection, not a loss of a request's answer.
    for id in requests {
        let message = f.worker.state.request(*id).unwrap().prompt.id;
        let status = f.worker.control_message_at(message, io::now_ms()).unwrap();
        assert_eq!(status["complete"], true);
        assert_eq!(status["requests"][0]["stage"], "replied");
        assert_eq!(
            status["requests"][0]["reply"]["text"],
            "SHARED_BATCH_ANSWER"
        );
        assert!(status["requests"][0].get("answered_with_message").is_none());
    }
}

#[test]
fn batch_history_projects_one_body_and_retains_full_status_after_archive_and_reopen() {
    let (mut f, requests) = batch_history_fixture();
    batch_history_assert_projection(&mut f, &requests);
    let completed_at = f
        .worker
        .state
        .request(requests[0])
        .unwrap()
        .completed_at_ms
        .unwrap();
    f.worker
        .archive_history(completed_at + crate::messaging::model::ARCHIVE_AFTER_MS + 1)
        .unwrap();
    f.reopen();
    batch_history_assert_projection(&mut f, &requests);
}

#[test]
fn batch_history_page_without_the_lead_keeps_a_stable_answer_reference() {
    let (mut f, requests) = batch_history_fixture();
    let messages: Vec<_> = requests
        .iter()
        .map(|id| f.worker.state.request(*id).unwrap().prompt.id)
        .collect();
    for (before, expected) in [(None, messages[2]), (Some(messages[2]), messages[1])] {
        let mut params = json!({"room":f.room.0.to_string(),"limit":1});
        if let Some(before) = before {
            params["before"] = json!(before);
        }
        let response = f.call("room.history", params);
        assert!(response.ok, "{response:?}");
        let page = &response.result;
        assert_eq!(page["messages"].as_array().unwrap().len(), 1);
        assert_eq!(page["messages"][0]["prompt"]["id"], json!(expected));
        let request = &page["messages"][0]["delivery"]["requests"][0];
        assert!(
            request["reply"].is_null(),
            "off-page lead must not duplicate body"
        );
        assert_eq!(request["answered_with_message"], json!(messages[0]));
        assert_eq!(page["next_before"], json!(expected));
    }
}

#[test]
fn batch_history_equal_text_in_an_independent_turn_keeps_its_own_body() {
    let (mut f, requests) = batch_history_fixture();
    let next = f.send(Author::Developer, "independent task");
    f.finish(next, "SHARED_BATCH_ANSWER");
    let response = f.call("room.history", json!({"room":f.room.0.to_string()}));
    assert!(response.ok, "{response:?}");
    let messages = response.result["messages"].as_array().unwrap();
    let bodies: Vec<_> = messages
        .iter()
        .filter(|message| {
            message["delivery"]["requests"][0]["reply"]["text"] == "SHARED_BATCH_ANSWER"
        })
        .map(|message| message["prompt"]["id"].clone())
        .collect();
    assert_eq!(
        bodies,
        [
            json!(f.worker.state.request(requests[0]).unwrap().prompt.id),
            json!(f.worker.state.request(next).unwrap().prompt.id),
        ]
    );
}
