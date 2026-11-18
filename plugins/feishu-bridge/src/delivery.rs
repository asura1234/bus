use crate::{
    config::Config,
    error::Result,
    process::{self, Control},
    store::Store,
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};

pub fn dispatch(
    store: &Store,
    c: &Config,
    ctl: &Control,
    retry: &mut HashMap<String, Instant>,
) -> Result<()> {
    let mut blocked = HashSet::new();
    for (seq, v) in store.pending(&c.app_id)? {
        if ctl.done() {
            break;
        }
        let chat = v["chat_id"].as_str().unwrap().to_owned();
        if blocked.contains(&chat) {
            continue;
        }
        if !c.permits(&v) || retry.get(&chat).is_some_and(|t| Instant::now() < *t) {
            blocked.insert(chat);
            continue;
        }
        let input = format!("{v}\n").into_bytes();
        match process::run(
            Path::new(&c.sink_command[0]),
            &c.sink_command[1..],
            input,
            c.sink_timeout_seconds,
            ctl,
            false,
        ) {
            Ok((status, _)) if status.success() => {
                store.ack(seq)?;
                retry.remove(&chat);
            }
            _ => {
                retry.insert(
                    chat.clone(),
                    Instant::now() + Duration::from_secs(c.retry_seconds),
                );
                blocked.insert(chat);
                eprintln!("sink did not acknowledge; message retained");
            }
        }
    }
    Ok(())
}
