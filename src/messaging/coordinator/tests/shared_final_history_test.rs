//! Developer history projects one body per shared final in a room.
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
    let response = f.call("room.history", json!({"room":f.room.0.to_string()}));
    assert!(response.ok, "{response:?}");
    let rendered = response.result.to_string();
    assert_eq!(
        rendered.matches("SHARED_BATCH_ANSWER").count(),
        1,
        "{rendered}"
    );
    let messages = response.result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    for (index, message) in messages.iter().enumerate() {
        assert_eq!(
            message["prompt"]["text"],
            ["first task", "second task", "third task"][index]
        );
        assert_eq!(message["delivery"]["complete"], true);
        let request = &message["delivery"]["requests"][0];
        if index == 0 {
            assert_eq!(request["stage"], "replied");
            assert_eq!(request["reply"]["text"], "SHARED_BATCH_ANSWER");
        } else {
            assert_eq!(request["stage"], "ended");
            assert!(request["reply"].is_null(), "{request}");
        }
        assert!(request["group"].is_null(), "{request}");
        assert!(request.get("answered_with_message").is_none());
        assert_ne!(request["reply_ack"], true, "{request}");
    }
    // Status stays lossless for whatever body the test helper stored.
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
fn batch_history_page_without_the_lead_does_not_repeat_the_answer() {
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
        assert!(request["reply"].is_null(), "{request}");
        assert!(request.get("answered_with_message").is_none(), "{request}");
        assert_eq!(request["stage"], "ended");
        assert!(
            !page.to_string().contains("SHARED_BATCH_ANSWER"),
            "history page contains the shared final: {page}"
        );
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
    let rendered = response.result.to_string();
    assert_eq!(
        rendered.matches("SHARED_BATCH_ANSWER").count(),
        2,
        "{rendered}"
    );
    let messages = response.result["messages"].as_array().unwrap();
    let ids: Vec<_> = messages
        .iter()
        .map(|message| message["prompt"]["id"].clone())
        .collect();
    assert!(ids.contains(&json!(
        f.worker.state.request(requests[0]).unwrap().prompt.id
    )));
    assert!(ids.contains(&json!(f.worker.state.request(next).unwrap().prompt.id)));
}

#[test]
fn identical_developer_post_and_final_appear_once_after_archive_and_reopen() {
    let mut f = Fixture::new();
    let id = f.send(Author::Developer, "question");
    let now = io::now_ms();
    let mut state = f.worker.state.clone();
    state
        .post_to_developer(f.room, f.agent, "ANSWER ALREADY POSTED".into(), vec![], now)
        .unwrap();
    state.finish_for_test(id, "ANSWER ALREADY POSTED", now + 1);
    f.worker.save(state).unwrap();
    f.worker
        .archive_history(now + crate::messaging::model::ARCHIVE_AFTER_MS + 2)
        .unwrap();
    f.reopen();

    let result = f.call("room.history", json!({"room":f.room.0.to_string()}));
    assert!(result.ok, "{result:?}");
    let messages = result.result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["prompt"]["text"], "ANSWER ALREADY POSTED");
    assert_eq!(
        result
            .result
            .to_string()
            .matches("ANSWER ALREADY POSTED")
            .count(),
        1
    );
}
