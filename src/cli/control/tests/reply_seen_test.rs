use super::*;
use std::sync::{atomic::AtomicBool, Arc};

struct Output {
    bytes: Vec<u8>,
    flushed: Arc<AtomicBool>,
    fail: bool,
}

#[test]
fn printed_outcome_acknowledges_only_the_event_keys_in_flushed_output() {
    for fail in [false, true] {
        let flushed = Arc::new(AtomicBool::new(false));
        let mut output = Output {
            bytes: vec![],
            flushed: flushed.clone(),
            fail,
        };
        let mut seen = Vec::new();
        let args = ["wait", "--message", "19", "--as", "orch"].map(str::to_owned);
        let _ = run_with_pause(
            &args,
            &mut output,
            &mut |request, _| {
                if request.method == "message.seen" {
                    assert!(flushed.load(Ordering::SeqCst));
                    seen.push(request.params.clone());
                    return Ok(Response::success(&request.id, json!({"seen":true})));
                }
                let mut response = Response::failure(&request.id, "message_abandoned", "abandoned");
                response.result = json!({"message_id":19,"complete":true,"requests":[
                    {"request_id":20,"stage":"abandoned","event_acks":["outcome:20"],"reply_ack":false}
                ]});
                Ok(response)
            },
            |_| panic!("no polling necessary"),
        );
        assert_eq!(seen.len(), usize::from(!fail));
        if !fail {
            assert_eq!(
                seen[0],
                json!({"request":"20","as":"orch","events":["outcome:20"]})
            );
        }
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.fail {
            return Err(io::Error::other("broken output"));
        }
        self.flushed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn reply_seen_only_after_author_waiter_prints_and_flushes() {
    for fail in [false, true] {
        let flushed = Arc::new(AtomicBool::new(false));
        let mut output = Output {
            bytes: vec![],
            flushed: flushed.clone(),
            fail,
        };
        let mut acknowledged = false;
        let args = ["wait", "--message", "19", "--as", "orch"].map(str::to_owned);
        let result = run_with_pause(
            &args,
            &mut output,
            &mut |request, _| {
                if request.method == "message.seen" {
                    assert!(flushed.load(Ordering::SeqCst));
                    assert_eq!(request.params, json!({"request":"20","as":"orch"}));
                    acknowledged = true;
                    return Err("ack connection lost; fallback must remain possible".into());
                }
                assert_eq!(request.params["as"], "orch");
                let mut response = Response::failure(&request.id, "timeout", "wait deadline");
                response.result = json!({"message_id":19,"complete":false,"requests":[
                    {"request_id":20,"stage":"replied","reply_ack":true,"reply":{"text":"built"}},
                    {"request_id":21,"stage":"queued","reply_ack":false}
                ]});
                Ok(response)
            },
            |_| panic!("no polling necessary"),
        );
        assert_eq!(
            result.unwrap_err().to_string(),
            if fail {
                "broken output"
            } else {
                "wait deadline"
            }
        );
        assert_eq!(acknowledged, !fail);
        assert!(String::from_utf8(output.bytes).unwrap().contains("built"));
    }
}

#[test]
fn reply_seen_plain_wait_and_status_do_not_ack_for_the_orchestrator() {
    for args in [
        vec!["wait", "--message", "19"],
        vec!["message", "status", "19"],
    ] {
        let mut calls = 0;
        run_with_pause(&args.into_iter().map(str::to_owned).collect::<Vec<_>>(), &mut Vec::new(), &mut |request, _| {
            calls += 1;
            assert_eq!(request.method, "message.status");
            Ok(Response::success(&request.id, json!({"message_id":19,"complete":true,"requests":[{"request_id":20,"reply_ack":true,"stage":"replied","reply":{"text":"built"}}]})))
        }, |_| panic!("no polling necessary")).unwrap();
        assert_eq!(calls, 1);
    }
}

#[test]
fn reply_seen_send_async_preserves_the_author_and_prints_the_reply() {
    let args = [
        "send", "--room", "work", "--as", "orch", "--to", "builder", "--async", "--text", "build",
    ]
    .map(str::to_owned);
    let mut calls = Vec::new();
    let mut output = Vec::new();
    run_with_pause(&args, &mut output, &mut |request, _| {
        calls.push(request.method.clone());
        Ok(Response::success(&request.id, match request.method.as_str() {
            "message.send" => json!({"message_id":19,"request_ids":[20]}),
            "message.status" => {
                assert_eq!(request.params, json!({"message":"19","as":"orch"}));
                json!({"message_id":19,"complete":true,"requests":[{"request_id":20,"stage":"replied","turn_ended":true,"reply_ack":true,"reply":{"text":"built successfully"}}]})
            },
            "message.seen" => json!({"seen":true}),
            other => panic!("unexpected {other}"),
        }))
    }, |_| panic!("no polling necessary")).unwrap();
    assert_eq!(calls, ["message.send", "message.status", "message.seen"]);
    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        response["result"]["requests"][0]["reply"]["text"],
        "built successfully"
    );
    assert_eq!(response["result"]["send_receipt"]["message_id"], 19);
}

#[test]
fn printed_reply_with_outcomes_explicitly_acknowledges_both() {
    let mut output = Vec::new();
    let mut ack = None;
    let args = ["wait", "--message", "19", "--as", "orch"].map(str::to_owned);
    run_with_pause(&args,&mut output,&mut |request,_| {
        if request.method=="message.seen" {
            ack=Some(request.params.clone());
            return Ok(Response::success(&request.id,json!({"seen":true})));
        }
        Ok(Response::success(&request.id,json!({"message_id":19,"complete":true,"requests":[{"request_id":20,"stage":"replied","reply_ack":true,"reply":{"text":"final"},"event_acks":["idle:20"]}]})))
    }, |_|panic!("no polling necessary")).unwrap();
    assert_eq!(
        ack,
        Some(json!({"request":"20","as":"orch","events":["idle:20"],"reply":true}))
    );
}
