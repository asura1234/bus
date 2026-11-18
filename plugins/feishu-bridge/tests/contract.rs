use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};
use tempfile::TempDir;

struct Harness {
    dir: TempDir,
}

impl Harness {
    fn new() -> Self {
        let h = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        h.script("lark", r#"#!/usr/bin/env python3
import json,sys,os
args=sys.argv[1:]
if args==['--version']:
 print('lark-cli version 1.0.97'); sys.exit(0)
if args==['auth','status','--json']:
 print(json.dumps({'appId':'cli_test','identities':{'bot':{'status':'ready','available':True}}})); sys.exit(0)
if args[:2]==['im','+messages-send']:
 assert args[args.index('--as')+1]=='bot'
 assert args[args.index('--text')+1]=='-'
 text=sys.stdin.read()
 assert text=='@literal $(never execute)\n雪'
 assert args[args.index('--idempotency-key')+1]=='stable-key'
 assert args[args.index('--user-id')+1]=='ou_allowed'
 print(json.dumps({'ok':True,'data':{'message_id':'om_sent','chat_id':'oc_allowed'}})); sys.exit(0)
raise AssertionError(args)
"#);
        h.script(
            "sink",
            r#"#!/usr/bin/env python3
import sys,json,os
v=json.load(sys.stdin)
with open(os.path.join(os.path.dirname(__file__),'received'),'a') as f: f.write(json.dumps(v)+'\n')
"#,
        );
        h.config("");
        h
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }
    fn script(&self, name: &str, content: &str) {
        fs::write(self.path(name), content).unwrap();
        fs::set_permissions(self.path(name), fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn config(&self, extra: &str) {
        fs::write(self.path("config.toml"), format!(
            "app_id = \"cli_test\"\nlark_cli = {lark:?}\nstate_dir = {state:?}\nallowed_sender_open_ids = [\"ou_allowed\"]\nallowed_chat_ids = [\"oc_allowed\",\"oc_other\"]\nsink_command = [{sink:?}]\nretry_seconds = 1\ncommand_timeout_seconds = 2\nsink_timeout_seconds = 1\nreconcile_seconds = 60\n{extra}",
            lark=self.path("lark"), state=self.path("state"), sink=self.path("sink"))).unwrap();
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(
            std::env::var_os("FEISHU_BRIDGE_TEST_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_feishu-bridge").into()),
        )
        .arg("--config")
        .arg(self.path("config.toml"))
        .args(args)
        .env("HOME", self.dir.path())
        .env("XDG_CONFIG_HOME", self.path("config"))
        .output()
        .unwrap()
    }
    fn import(&self, events: &[Value]) -> Output {
        fs::write(
            self.path("events"),
            events.iter().map(|v| format!("{v}\n")).collect::<String>(),
        )
        .unwrap();
        self.run(&["listen", "--input", self.path("events").to_str().unwrap()])
    }
    fn received(&self) -> Vec<Value> {
        fs::read_to_string(self.path("received"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
    fn status(&self) -> Value {
        success_json(self.run(&["status"]))
    }
}

fn success_json(o: Output) -> Value {
    assert!(
        o.status.success(),
        "status {:?}, stderr {}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap()
}

fn event(id: &str, chat: &str, time: &str) -> Value {
    json!({"type":"im.message.receive_v1","event_id":"ev_example","message_id":id,
        "chat_id":chat,"chat_type":"p2p","sender_id":"ou_allowed","sender_type":"user",
        "message_type":"text","content":"{\"text\":\"already decoded\"}","create_time":time,
        "reply_to":"om_parent","root_id":"om_root","thread_id":""})
}

#[test]
fn send_preserves_literal_text_and_returns_external_ids() {
    let h = Harness::new();
    let v = success_json(h.run(&[
        "send",
        "--open-id",
        "ou_allowed",
        "--text",
        "@literal $(never execute)\n雪",
        "--idempotency-key",
        "stable-key",
    ]));
    assert_eq!(v, json!({"message_id":"om_sent","chat_id":"oc_allowed"}));
}

#[test]
fn app_mismatch_prevents_send() {
    let h = Harness::new();
    let s = fs::read_to_string(h.path("config.toml"))
        .unwrap()
        .replace("cli_test", "cli_wrong");
    fs::write(h.path("config.toml"), s).unwrap();
    assert_eq!(
        h.run(&["send", "--open-id", "ou_allowed", "--text", "never send"])
            .status
            .code(),
        Some(3)
    );
}

#[test]
fn old_lark_cli_version_blocks_send_with_a_clear_error() {
    let h = Harness::new();
    h.script(
        "lark",
        r#"#!/usr/bin/env python3
import sys
if sys.argv[1:]==['--version']:
 print('lark-cli version 1.0.95'); sys.exit(0)
raise AssertionError('send must not run on an old lark-cli')
"#,
    );
    let out = h.run(&["send", "--open-id", "ou_allowed", "--text", "hello"]);
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("1.0.95"), "stderr: {err}");
    assert!(err.contains("1.0.97"), "stderr: {err}");
}

#[test]
fn import_normalizes_text_and_deduplicates_across_process_restarts() {
    let h = Harness::new();
    let e = event("om_one", "oc_allowed", "1000");
    assert!(h.import(&[e.clone(), e.clone()]).status.success());
    assert!(h.import(&[e]).status.success());
    assert_eq!(
        h.received(),
        vec![
            json!({"app_id":"cli_test","message_id":"om_one","chat_id":"oc_allowed",
        "chat_type":"p2p","sender":{"open_id":"ou_allowed","sender_type":"user"},
        "text":"{\"text\":\"already decoded\"}","reply_to":"om_parent","root_id":"om_root",
        "thread_id":null,"create_time":"1000"})
        ]
    );
    assert_eq!(h.status()["delivered"], 1);
    assert_eq!(h.status()["pending"], 0);
}

#[test]
fn import_rejects_names_bots_groups_nontext_and_nonallowlisted_identities() {
    let h = Harness::new();
    let mutations = [
        ("sender_id", json!("ou_stranger")),
        ("chat_id", json!("oc_stranger")),
        ("sender_type", json!("bot")),
        ("chat_type", json!("group")),
        ("message_type", json!("post")),
        ("sender_id", Value::Null),
    ];
    let events = mutations
        .iter()
        .enumerate()
        .map(|(i, (key, v))| {
            let mut e = event(&format!("om_{i}"), "oc_allowed", "1000");
            e[*key] = v.clone();
            e["sender_name"] = json!("ou_allowed");
            e
        })
        .collect::<Vec<_>>();
    assert!(h.import(&events).status.success());
    assert!(h.received().is_empty());
    assert_eq!(h.status()["pending"], 0);
}

#[test]
fn failed_head_blocks_its_chat_while_other_chat_delivers_and_replay_recovers() {
    let h = Harness::new();
    h.script("sink",r#"#!/usr/bin/env python3
import json,sys,os
v=json.load(sys.stdin)
if v['message_id']=='om_first' and not os.path.exists(os.path.join(os.path.dirname(__file__),'unblock')): sys.exit(7)
with open(os.path.join(os.path.dirname(__file__),'received'),'a') as f:f.write(json.dumps(v)+'\n')
"#);
    assert_eq!(
        h.import(&[
            event("om_first", "oc_allowed", "2000"),
            event("om_second", "oc_allowed", "1000"),
            event("om_other", "oc_other", "3000")
        ])
        .status
        .code(),
        Some(1)
    );
    assert_eq!(
        h.received()
            .iter()
            .map(|e| e["message_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["om_other"]
    );
    assert_eq!(h.status()["pending"], 2);
    fs::write(h.path("unblock"), "").unwrap();
    assert!(h.import(&[]).status.success());
    assert_eq!(
        h.received()
            .iter()
            .map(|e| e["message_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["om_other", "om_first", "om_second"]
    );
}

#[test]
fn revoked_pending_sender_is_parked_on_restart() {
    let h = Harness::new();
    h.script("sink", "#!/bin/sh\nexit 1\n");
    assert_eq!(
        h.import(&[event("om_park", "oc_allowed", "1000")])
            .status
            .code(),
        Some(1)
    );
    let s = fs::read_to_string(h.path("config.toml"))
        .unwrap()
        .replace("ou_allowed", "ou_replacement");
    fs::write(h.path("config.toml"), s).unwrap();
    h.script("sink", "#!/bin/sh\ntouch revoked-was-delivered\nexit 0\n");
    assert_eq!(h.import(&[]).status.code(), Some(1));
    assert_eq!(h.status()["pending"], 1);
}

#[test]
fn invalid_authorized_timestamp_fails_without_silently_dropping() {
    let h = Harness::new();
    assert_eq!(
        h.import(&[event("om_bad", "oc_allowed", "yesterday")])
            .status
            .code(),
        Some(1)
    );
    assert!(h.received().is_empty());
}

#[test]
fn sink_timeout_keeps_durable_pending() {
    let h = Harness::new();
    h.script("sink", "#!/bin/sh\nsleep 10\n");
    let before = std::time::Instant::now();
    assert_eq!(
        h.import(&[event("om_timeout", "oc_allowed", "1000")])
            .status
            .code(),
        Some(1)
    );
    assert!(before.elapsed().as_secs() < 5);
    assert_eq!(h.status()["pending"], 1);
}

#[test]
fn invalid_config_and_recipient_combinations_have_exit_two() {
    let h = Harness::new();
    h.config("unknown_setting = true\n");
    assert_eq!(h.run(&["status"]).status.code(), Some(2));
    h.config("");
    assert_eq!(
        h.run(&[
            "send",
            "--open-id",
            "ou_allowed",
            "--chat-id",
            "oc_allowed",
            "--text",
            "no"
        ])
        .status
        .code(),
        Some(2)
    );
    assert_eq!(
        h.run(&[
            "send",
            "--open-id",
            "ou_allowed",
            "--text",
            "no",
            "--idempotency-key",
            ""
        ])
        .status
        .code(),
        Some(2)
    );
}

#[test]
fn state_is_created_with_private_permissions() {
    let h = Harness::new();
    assert!(h.import(&[]).status.success());
    for (path, mask) in [
        (h.path("state"), 0o077),
        (h.path("state/spool.sqlite3"), 0o077),
    ] {
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & mask, 0);
    }
}

#[test]
fn status_and_plist_lifecycle_preserve_state_without_loading_launchd() {
    let h = Harness::new();
    let s = h.status();
    assert_eq!(s["running"], false);
    assert_eq!(s["pending"], 0);
    assert_eq!(s["installed"], false);
    let install = success_json(h.run(&["install"]));
    let path = std::path::Path::new(install["plist_path"].as_str().unwrap());
    assert!(path.starts_with(h.dir.path()));
    let bytes = fs::read(path).unwrap();
    let parsed = Command::new("/usr/bin/plutil")
        .args(["-lint"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        parsed.status.success(),
        "{}",
        String::from_utf8_lossy(&parsed.stdout)
    );
    assert!(String::from_utf8_lossy(&bytes).contains("KeepAlive"));
    let xml = Command::new("/usr/bin/plutil")
        .args(["-extract", "EnvironmentVariables.PATH", "raw", "-o", "-"])
        .arg(path)
        .output()
        .unwrap();
    assert!(xml.status.success());
    assert!(String::from_utf8_lossy(&xml.stdout)
        .split(':')
        .any(|p| p == "/opt/homebrew/bin"));
    assert_eq!(h.status()["installed"], true);
    assert!(h
        .import(&[event("om_keep", "oc_allowed", "1000")])
        .status
        .success());
    success_json(h.run(&["uninstall"]));
    assert!(!path.exists());
    assert_eq!(h.status()["delivered"], 1);
    success_json(h.run(&["uninstall"]));
}

impl Harness {
    fn fake_history(&self, scenario: &str) {
        fs::write(self.path("scenario"), scenario).unwrap();
        self.script("lark",r#"#!/usr/bin/env python3
import json,sys,os,time
base=os.path.dirname(__file__)
args=sys.argv[1:]
scenario=open(os.path.join(base,'scenario')).read()
def out(v): print(json.dumps(v));sys.exit(0)
if args==['auth','status','--json']:
 out({'appId':'cli_test','identities':{'bot':{'status':'ready','available':True}}})
if args[:3]==['im','chats','get']:
 assert args[args.index('--as')+1]=='bot'
 out({'ok':True,'data':{'chat_mode':'group' if scenario=='group' else 'p2p'}})
if args[:3]==['api','GET','/open-apis/im/v1/messages/om_sent']:
 if scenario=='missing-root':sys.exit(1)
 if scenario=='slow-root':time.sleep(10)
 with open(os.path.join(base,'root-queries'),'a') as f:f.write('root\n')
 out({'ok':True,'data':{'items':[{'message_id':'om_sent','chat_id':'oc_allowed','msg_type':'text','create_time':'100','sender':{'id':'cli_test','id_type':'app_id','sender_type':'app'},'body':{'content':'{"text":"outbound"}'},'thread_id':'omt_old'}]}})
if args[:3]==['event','consume','im.message.receive_v1']:
 assert args[args.index('--as')+1]=='bot'
 print('[event] ready event_key=im.message.receive_v1',file=sys.stderr,flush=True)
 if scenario in ['live','missing-root','slow-root','recalled']:
  print(json.dumps({'type':'im.message.receive_v1','message_id':'om_live','chat_id':'oc_allowed','chat_type':'p2p','sender_id':'ou_allowed','sender_type':'user','message_type':'text','content':'live literal','create_time':'9000'}),flush=True)
 time.sleep(15);sys.exit(0)
if args[:3]==['api','GET','/open-apis/im/v1/messages']:
 assert args[args.index('--as')+1]=='bot'
 p=json.loads(args[args.index('--params')+1])
 with open(os.path.join(base,'queries'),'a') as f:f.write(json.dumps(p)+'\n')
 def msg(i,t,text):return {'message_id':i,'chat_id':'oc_allowed','msg_type':'text','create_time':t,'sender':{'id':'ou_allowed','id_type':'open_id','sender_type':'user'},'body':{'content':json.dumps({'text':text})}}
 if p['container_id']=='oc_other':out({'ok':True,'data':{'items':[],'has_more':False}})
 if scenario=='recalled':out({'ok':True,'data':{'items':[dict(msg('om_recalled','1000','old'),deleted=True,body={'content':'This message was recalled'})],'has_more':False}})
 if p['container_id_type']=='thread':
  assert 'start_time' not in p and 'end_time' not in p
  if not p.get('page_token'):out({'ok':True,'data':{'items':[dict(msg('om_child','3000','thread reply'),parent_id='om_root',root_id='om_root',thread_id='omt_old')],'has_more':True,'page_token':'thread-next'}})
  assert p['page_token']=='thread-next'
  out({'ok':True,'data':{'items':[dict(msg('om_child2','4000','thread second'),parent_id='om_child',root_id='om_root',thread_id='omt_old')],'has_more':False}})
 if scenario in ['empty','live','root','slow-root']:out({'ok':True,'data':{'items':[],'has_more':False}})
 if not p.get('page_token'):
  m=msg('om_later','2000','later 雪')
  if scenario=='thread':m['thread_id']='omt_old'
  out({'ok':True,'data':{'items':[m],'has_more':True,'page_token':'next'}})
 assert p['page_token']=='next'
 if scenario=='partial':sys.exit(4)
 if scenario=='repeat':out({'ok':True,'data':{'items':[],'has_more':True,'page_token':'next'}})
 out({'ok':True,'data':{'items':[msg('om_earlier','1000','earlier')],'has_more':False}})
raise AssertionError(args)
"#);
    }
    fn queries(&self) -> Vec<Value> {
        fs::read_to_string(self.path("queries"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
}

#[test]
fn recovery_exhausts_pages_sorts_backfill_and_advances_overlapping_watermark() {
    let h = Harness::new();
    h.fake_history("pages");
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    assert_eq!(
        h.received()
            .iter()
            .map(|e| e["message_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["om_earlier", "om_later"]
    );
    assert_eq!(h.received()[1]["text"], "later 雪");
    assert!(
        h.status()["watermarks"]["oc_allowed"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            > 1000
    );
    let watermark = h.status()["watermarks"]["oc_allowed"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    h.fake_history("empty");
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    let queries = h.queries();
    let starts = queries
        .iter()
        .filter(|q| q["container_id"] == "oc_allowed" && !q["page_token"].is_string())
        .map(|q| q["start_time"].as_str().unwrap().parse::<i64>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(starts[0], 0);
    assert_eq!(starts[1], (watermark - 300_000) / 1000);
}

#[test]
fn partial_or_repeated_pagination_never_advances_watermark_or_dispatches_half_batch() {
    for scenario in ["partial", "repeat"] {
        let h = Harness::new();
        h.fake_history(scenario);
        assert!(h.run(&["listen", "--run-for", "2"]).status.success());
        assert!(h.status()["watermarks"].get("oc_allowed").is_none());
        assert!(h.received().is_empty());
        h.fake_history("pages");
        assert!(h.run(&["listen", "--run-for", "2"]).status.success());
        assert_eq!(h.received().len(), 2);
    }
}

#[test]
fn remembered_old_threads_are_fully_revisited_outside_chat_watermark() {
    let h = Harness::new();
    h.fake_history("thread");
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    assert_eq!(h.received().len(), 4);
    h.fake_history("empty");
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    assert_eq!(h.received().len(), 4);
    assert_eq!(
        h.queries()
            .iter()
            .filter(|q| q["container_id_type"] == "thread")
            .count(),
        4
    );
}

#[test]
fn consumer_spools_while_sink_down_then_restart_delivers_once() {
    let h = Harness::new();
    h.fake_history("live");
    h.script("sink", "#!/bin/sh\nexit 1\n");
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    assert_eq!(h.status()["pending"], 1);
    h.script(
        "sink",
        r#"#!/usr/bin/env python3
import sys,json,os
v=json.load(sys.stdin)
with open(os.path.join(os.path.dirname(__file__),'received'),'a') as f:f.write(json.dumps(v)+'\n')
"#,
    );
    h.fake_history("empty");
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    assert_eq!(h.received().len(), 1);
    assert_eq!(h.status()["pending"], 0);
}

#[test]
fn app_lock_excludes_a_second_listener_even_with_another_state_dir() {
    let h = Harness::new();
    h.fake_history("empty");
    let mut first = Command::new(env!("CARGO_BIN_EXE_feishu-bridge"))
        .arg("--config")
        .arg(h.path("config.toml"))
        .args(["listen", "--run-for", "4"])
        .env("HOME", h.dir.path())
        .env("XDG_CONFIG_HOME", h.path("config"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !h.status()["running"].as_bool().unwrap_or(false) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let s = fs::read_to_string(h.path("config.toml"))
        .unwrap()
        .replace("/state\"", "/other-state\"");
    fs::write(h.path("other.toml"), s).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_feishu-bridge"))
        .arg("--config")
        .arg(h.path("other.toml"))
        .args(["listen", "--input"])
        .arg(h.path("empty"))
        .env("HOME", h.dir.path())
        .env("XDG_CONFIG_HOME", h.path("different-config-root"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert!(first.wait().unwrap().success());
}

#[test]
fn raw_history_cannot_bypass_group_refusal_with_allowlisted_ids() {
    let h = Harness::new();
    h.fake_history("group");
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    assert!(h.received().is_empty());
    assert!(h.status()["watermarks"].get("oc_allowed").is_none());
}

#[test]
fn old_outbound_roots_discover_threads_without_new_chat_history() {
    let h = Harness::new();
    success_json(h.run(&[
        "send",
        "--open-id",
        "ou_allowed",
        "--text",
        "@literal $(never execute)\n雪",
        "--idempotency-key",
        "stable-key",
    ]));
    h.fake_history("root");
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    assert_eq!(
        h.received()
            .iter()
            .map(|e| e["message_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["om_child", "om_child2"]
    );
}

#[test]
fn edit_replay_keeps_first_version_and_dedupe_key_is_app_scoped() {
    let h = Harness::new();
    h.script("sink", "#!/bin/sh\nexit 1\n");
    let e = event("om_edit", "oc_allowed", "1000");
    assert_eq!(h.import(std::slice::from_ref(&e)).status.code(), Some(1));
    let mut edited = e.clone();
    edited["content"] = json!("changed");
    assert_eq!(h.import(&[edited]).status.code(), Some(1));
    h.script(
        "sink",
        r#"#!/usr/bin/env python3
import sys,json,os
v=json.load(sys.stdin)
with open(os.path.join(os.path.dirname(__file__),'received'),'a') as f:f.write(json.dumps(v)+'\n')
"#,
    );
    assert!(h.import(&[]).status.success());
    assert_eq!(h.received()[0]["text"], "{\"text\":\"already decoded\"}");
    let config = fs::read_to_string(h.path("config.toml"))
        .unwrap()
        .replace("cli_test", "cli_second");
    fs::write(h.path("config.toml"), config).unwrap();
    assert!(h.import(&[e]).status.success());
    assert_eq!(h.received().len(), 2);
    assert_eq!(h.received()[1]["app_id"], "cli_second");
    assert_eq!(h.status()["delivered"], 1);
}

#[test]
fn sink_can_deduplicate_acceptance_after_missing_acknowledgement() {
    let h = Harness::new();
    h.script(
        "sink",
        r#"#!/usr/bin/env python3
import sys,json,os
v=json.load(sys.stdin);p=os.path.join(os.path.dirname(__file__),'effect')
if os.path.exists(p):sys.exit(0)
with open(p,'w') as f:f.write(v['message_id']);f.flush();os.fsync(f.fileno())
sys.exit(9)
"#,
    );
    assert_eq!(
        h.import(&[event("om_effect", "oc_allowed", "1000")])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(fs::read_to_string(h.path("effect")).unwrap(), "om_effect");
    assert!(h.import(&[]).status.success());
    assert_eq!(h.status()["delivered"], 1);
}

#[test]
fn event_without_readiness_is_not_delivered_even_after_consumer_exits() {
    let h = Harness::new();
    h.fake_history("empty");
    let fake = fs::read_to_string(h.path("lark"))
        .unwrap()
        .replace(
            "print('[event] ready event_key=im.message.receive_v1',file=sys.stderr,flush=True)",
            "pass",
        )
        .replace("if scenario=='live':", "if scenario=='empty':")
        .replace("time.sleep(15);sys.exit(0)", "sys.exit(0)");
    fs::write(h.path("lark"), fake).unwrap();
    assert!(h.run(&["listen", "--run-for", "2"]).status.success());
    assert!(h.received().is_empty());
    assert_eq!(h.status()["pending"], 0);
}

#[test]
fn chat_send_without_key_preserves_literal_dash_and_does_not_retry_failure() {
    let h = Harness::new();
    h.script("lark",r#"#!/usr/bin/env python3
import sys,json,os
args=sys.argv[1:]
if args==['--version']:
 print('lark-cli version 1.0.97'); sys.exit(0)
if args==['auth','status','--json']:
 print(json.dumps({'appId':'cli_test','identities':{'bot':{'status':'ready','available':True}}}));sys.exit(0)
assert args[:2]==['im','+messages-send']
assert args[args.index('--chat-id')+1]=='oc_allowed'
assert args[args.index('--as')+1]=='bot' and '--idempotency-key' not in args
assert sys.stdin.read()=='-literal'
p=os.path.join(os.path.dirname(__file__),'attempts')
with open(p,'a') as f:f.write('attempt\n')
if os.path.exists(os.path.join(os.path.dirname(__file__),'fail')):sys.exit(4)
print(json.dumps({'ok':True,'data':{'message_id':'om_chat','chat_id':'oc_allowed'}}))
"#);
    assert_eq!(
        success_json(h.run(&["send", "--chat-id", "oc_allowed", "--text", "-literal"])),
        json!({"message_id":"om_chat","chat_id":"oc_allowed"})
    );
    fs::write(h.path("fail"), "").unwrap();
    assert_eq!(
        h.run(&["send", "--chat-id", "oc_allowed", "--text", "-literal"])
            .status
            .code(),
        Some(3)
    );
    assert_eq!(
        fs::read_to_string(h.path("attempts")).unwrap(),
        "attempt\nattempt\n"
    );
}

#[test]
fn transport_spools_a_second_event_while_sink_is_still_blocked() {
    let h = Harness::new();
    h.fake_history("live");
    let fake=fs::read_to_string(h.path("lark")).unwrap().replace(" time.sleep(15);sys.exit(0)",
        " while not os.path.exists(os.path.join(base,'blocked')):time.sleep(0.01)\n print(json.dumps({'type':'im.message.receive_v1','message_id':'om_while_blocked','chat_id':'oc_allowed','chat_type':'p2p','sender_id':'ou_allowed','sender_type':'user','message_type':'text','content':'second','create_time':'9500'}),flush=True)\n time.sleep(15);sys.exit(0)");
    fs::write(h.path("lark"), fake).unwrap();
    let cfg = fs::read_to_string(h.path("config.toml"))
        .unwrap()
        .replace("sink_timeout_seconds = 1", "sink_timeout_seconds = 5");
    fs::write(h.path("config.toml"), cfg).unwrap();
    h.script(
        "sink",
        &format!("#!/bin/sh\ntouch {:?}\nsleep 10\n", h.path("blocked")),
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_feishu-bridge"))
        .arg("--config")
        .arg(h.path("config.toml"))
        .args(["listen", "--run-for", "3"])
        .env("HOME", h.dir.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while h.status()["pending"] != 2 && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(h.path("blocked").exists());
    assert_eq!(h.status()["pending"], 2);
    assert!(child.wait().unwrap().success());
    assert_eq!(h.status()["delivered"], 0);
}

#[test]
fn bounded_listener_exits_cleanly_even_during_slow_auth_startup() {
    let h = Harness::new();
    h.script("lark", "#!/bin/sh\nsleep 10\n");
    let start = std::time::Instant::now();
    let result = h.run(&["listen", "--run-for", "1"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(start.elapsed().as_secs() < 4);
}

#[test]
fn live_spool_delivery_continues_when_old_root_is_missing_or_slow() {
    for scenario in ["missing-root", "slow-root"] {
        let h = Harness::new();
        success_json(h.run(&[
            "send",
            "--open-id",
            "ou_allowed",
            "--text",
            "@literal $(never execute)\n雪",
            "--idempotency-key",
            "stable-key",
        ]));
        h.fake_history(scenario);
        assert!(h.run(&["listen", "--run-for", "3"]).status.success());
        assert_eq!(
            h.status()["delivered"],
            if scenario == "missing-root" { 3 } else { 1 },
            "{scenario}"
        );
        assert_eq!(h.status()["pending"], 0);
        assert!(h.status()["watermarks"].get("oc_allowed").is_none());
    }
}

#[test]
fn recalled_history_tombstone_is_skipped_without_wedging_watermark() {
    let h = Harness::new();
    h.fake_history("recalled");
    assert!(h.run(&["listen", "--run-for", "3"]).status.success());
    assert_eq!(h.status()["delivered"], 1);
    assert!(h.status()["watermarks"]["oc_allowed"].as_str().is_some());
    assert_eq!(h.received()[0]["message_id"], "om_live");
}

#[test]
fn resolved_root_is_replaced_by_durable_thread_tracking() {
    let h = Harness::new();
    success_json(h.run(&[
        "send",
        "--open-id",
        "ou_allowed",
        "--text",
        "@literal $(never execute)\n雪",
        "--idempotency-key",
        "stable-key",
    ]));
    h.fake_history("root");
    assert!(h.run(&["listen", "--run-for", "3"]).status.success());
    assert_eq!(h.received().len(), 2);
    h.fake_history("empty");
    assert!(h.run(&["listen", "--run-for", "3"]).status.success());
    assert_eq!(
        fs::read_to_string(h.path("root-queries")).unwrap(),
        "root\n"
    );
    assert_eq!(
        h.queries()
            .iter()
            .filter(|q| q["container_id_type"] == "thread")
            .count(),
        4
    );
}

#[test]
fn detached_sink_helper_cannot_hold_the_dispatcher_past_its_deadline() {
    let out = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/sink_process_timeout.py"
        ))
        .arg("SinkProcessTimeout.test_bounded_listener_exits_when_sink_descendant_retains_stdin")
        .env("FEISHU_BRIDGE_BINARY", env!("CARGO_BIN_EXE_feishu-bridge"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn status_observes_empty_spool_while_initial_schema_is_being_created() {
    let h = Harness::new();
    fs::create_dir(h.path("state")).unwrap();
    fs::write(h.path("state/spool.sqlite3"), "").unwrap();
    assert_eq!(h.status()["pending"], 0);
    assert_eq!(h.status()["delivered"], 0);
}
