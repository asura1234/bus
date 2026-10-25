//! A real scratch coordinator and control socket, without provider launches.
use super::*;
use crate::messaging::control;
use std::{sync::mpsc, thread, time::Duration};

#[test]
fn large_room_history_roundtrips_through_the_control_client() {
    let dir = crate::utils::test_temp::unique_temp_path("bhc");
    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    let room = worker.state.create_room("large").unwrap();
    let agent = worker
        .state
        .create_agent(room, "author", Provider::Codex, dir.clone(), None)
        .unwrap();
    let text = "history payload ".repeat(8192);
    let mut ids = Vec::new();
    for index in 0..96 {
        ids.push(
            worker
                .state
                .post_to_developer(room, agent, text.clone(), vec![], index)
                .unwrap(),
        );
    }
    let (commands, receiver) = mpsc::sync_channel(1);
    let server = control::server::start(&dir, commands).unwrap();
    let coordinator = thread::spawn(move || {
        let (_, BusCommand::Control(call)) =
            receiver.recv_timeout(Duration::from_secs(10)).unwrap()
        else {
            panic!("expected history control call")
        };
        let response = worker.control_response(&call.request);
        assert!(response.ok, "{response:?}");
        let size = serde_json::to_vec(&response).unwrap().len();
        assert!((10 * 1024 * 1024..16 * 1024 * 1024).contains(&size));
        call.reply.send(response).unwrap();
    });
    let result = control::request(
        &dir,
        &control::Request {
            id: "large-room-history".into(),
            method: "room.history".into(),
            params: json!({"room":room.0.to_string()}),
        },
    );
    coordinator.join().unwrap();
    drop(server);
    std::fs::remove_dir_all(dir).unwrap();
    let response = result.expect("large history must arrive as a complete response");
    assert!(response.ok, "{response:?}");
    let messages = response.result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), ids.len());
    for (message, id) in messages.iter().zip(ids) {
        assert_eq!(message["prompt"]["id"], json!(id));
        assert_eq!(message["prompt"]["text"], text);
    }
}

#[test]
fn history_pages_walk_back_without_gaps_or_duplicate_messages() {
    let (mut worker, room, agent, dir) = fixture();
    let mut ids = Vec::new();
    for index in 0..5 {
        let response = call(
            &mut worker,
            &format!("send-{index}"),
            "message.send",
            json!({"room":room.0.to_string(),"as":agent.0.to_string(),"to":["you"],"text":format!("message {index}")}),
        );
        assert!(response.ok, "{response:?}");
        ids.push(response.result["message_id"].clone());
    }
    let mut before = None;
    for expected in [&ids[3..5], &ids[1..3], &ids[0..1]] {
        let mut params = json!({"room":room.0.to_string(),"limit":2});
        if let Some(cursor) = before {
            params["before"] = cursor;
        }
        let response = call(&mut worker, "page", "room.history", params);
        assert!(response.ok, "{response:?}");
        let page = &response.result;
        let actual: Vec<_> = page["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["prompt"]["id"].clone())
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(page["has_more"], expected != &ids[0..1]);
        before = Some(page["next_before"].clone());
    }
    assert_eq!(before, Some(Value::Null));
    // No flags keeps the complete-history response contract.
    let complete = call(
        &mut worker,
        "complete",
        "room.history",
        json!({"room":room.0.to_string()}),
    );
    assert_eq!(
        complete.result["messages"].as_array().unwrap().len(),
        ids.len()
    );
    assert!(complete.result.get("has_more").is_none());
    for params in [
        json!({"limit":0}),
        json!({"limit":-1}),
        json!({"limit":"2"}),
        json!({"limit":2,"before":0}),
        json!({"before":ids[1]}),
    ] {
        let mut params = params;
        params["room"] = json!(room.0.to_string());
        assert!(!call(&mut worker, "bad-page", "room.history", params).ok);
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
