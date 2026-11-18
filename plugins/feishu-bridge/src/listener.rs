use crate::{
    config::Config,
    delivery,
    error::{Error, Result},
    history, model,
    process::{self, Control},
    store::{AppLock, Store},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

pub fn import(c: &Config, path: &Path, ctl: &Control) -> Result<()> {
    c.listener()?;
    let _lock = AppLock::acquire(c)?;
    if !path.is_absolute() {
        return Err(crate::error::invalid("input path must be absolute"));
    }
    let store = Store::open(c)?;
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event: Value =
            serde_json::from_str(&line).map_err(|_| Error(1, "invalid imported JSON".into()))?;
        if let Some(v) = model::normalize_event(
            &c.app_id,
            &c.allowed_sender_open_ids,
            &c.allowed_chat_ids,
            &event,
        )
        .map_err(|s| Error(1, s))?
        {
            let tx = store.conn.unchecked_transaction()?;
            store.insert(&v)?;
            tx.commit()?;
        }
    }
    delivery::dispatch(&store, c, ctl, &mut HashMap::new())?;
    if !store.pending(&c.app_id)?.is_empty() {
        return Err(Error(1, "deliveries remain pending".into()));
    }
    Ok(())
}

fn stream(c: Config, ctl: Control, tx: mpsc::Sender<Result<()>>) {
    while !ctl.done() {
        let args = [
            "event",
            "consume",
            "im.message.receive_v1",
            "--as",
            "bot",
            "--timeout",
            "300s",
        ]
        .map(String::from);
        let spawned = process::command(&c.lark_cli, &args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(_) => {
                let _ = tx.send(Ok(()));
                thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let session_ended = Arc::new(AtomicBool::new(false));
        let signal = ready.clone();
        let signals = tx.clone();
        let errors = thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if line == "[event] ready event_key=im.message.receive_v1" {
                    signal.store(true, Ordering::SeqCst);
                    eprintln!("[event] ready event_key=im.message.receive_v1");
                } else if line == "[source] feishu-websocket: connected" {
                    eprintln!("[source] feishu-websocket: connected");
                } else if line.to_ascii_lowercase().contains("drop") {
                    let _ = signals.send(Ok(()));
                    eprintln!("transport drop diagnostic; reconciling history");
                }
            }
        });
        let events = tx.clone();
        let cfg = c.clone();
        let stop = ctl.clone();
        let read_ready = ready.clone();
        let read_ended = session_ended.clone();
        let reader = thread::spawn(move || -> Result<()> {
            let store = Store::open(&cfg)?;
            for line in BufReader::new(stdout).lines() {
                let line = line?;
                if stop.done() {
                    break;
                }
                while !read_ready.load(Ordering::SeqCst)
                    && !read_ended.load(Ordering::SeqCst)
                    && !stop.done()
                {
                    thread::sleep(Duration::from_millis(5));
                }
                if !read_ready.load(Ordering::SeqCst) || stop.done() {
                    break;
                }
                let event: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = events.send(Ok(()));
                        continue;
                    }
                };
                match model::normalize_event(
                    &cfg.app_id,
                    &cfg.allowed_sender_open_ids,
                    &cfg.allowed_chat_ids,
                    &event,
                ) {
                    Ok(Some(v)) => {
                        let t = store.conn.unchecked_transaction()?;
                        store.insert(&v)?;
                        t.commit()?;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        let _ = events.send(Ok(()));
                        eprintln!("invalid authorized event; reconciling history");
                    }
                }
            }
            Ok(())
        });
        let until = Instant::now() + Duration::from_secs(310);
        let startup = Instant::now() + Duration::from_secs(c.command_timeout_seconds);
        while !ctl.done() && Instant::now() < until {
            if child.try_wait().ok().flatten().is_some() || reader.is_finished() {
                break;
            }
            if Instant::now() > startup && !ready.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        process::terminate(&mut child);
        let _ = errors.join();
        // A consumer exit unblocks waiting readers, but cannot forge readiness.
        session_ended.store(true, Ordering::SeqCst);
        match reader.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = tx.send(Err(e));
                return;
            }
            Err(_) => {
                let _ = tx.send(Err(Error(1, "event reader failed".into())));
                return;
            }
        }
        let _ = tx.send(Ok(()));
        for _ in 0..10 {
            if ctl.done() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

pub fn live(c: &Config, ctl: &Control) -> Result<()> {
    c.listener()?;
    let _lock = AppLock::acquire(c)?;
    if let Err(e) = process::verify(c, ctl) {
        if ctl.done() {
            return Ok(());
        }
        return Err(e);
    }
    let store = Store::open(c)?;
    let (events_tx, events_rx) = mpsc::channel();
    let (fatal_tx, fatal_rx) = mpsc::channel();
    let cfg = c.clone();
    let control = ctl.clone();
    let consumer = thread::spawn(move || stream(cfg, control, events_tx));
    let cfg = c.clone();
    let control = ctl.clone();
    let recovery = thread::spawn(move || {
        let result = (|| -> Result<()> {
            let mut store = Store::open(&cfg)?;
            while !control.done() {
                for chat in &cfg.allowed_chat_ids {
                    if control.done() {
                        break;
                    }
                    match history::reconcile(&mut store, &cfg, chat, &control) {
                        Ok(()) => {}
                        Err(e) if e.0 == 4 => return Err(e),
                        Err(e) => {
                            if !control.done() {
                                eprintln!("history incomplete; watermark retained: {e}");
                            }
                        }
                    }
                }
                let next = Instant::now() + Duration::from_secs(cfg.reconcile_seconds);
                while !control.done() && Instant::now() < next {
                    match events_rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(result) => {
                            result?;
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(Error(1, "event supervisor stopped".into()))
                        }
                    }
                }
            }
            Ok(())
        })();
        let _ = fatal_tx.send(result);
    });
    let result = (|| {
        let mut retry = HashMap::new();
        while !ctl.done() {
            if let Ok(result) = fatal_rx.try_recv() {
                result?;
                break;
            }
            delivery::dispatch(&store, c, ctl, &mut retry)?;
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    })();
    ctl.stop.store(true, Ordering::SeqCst);
    let consumer_ok = consumer.join().is_ok();
    let recovery_ok = recovery.join().is_ok();
    if !consumer_ok || !recovery_ok {
        return Err(Error(1, "listener worker failed".into()));
    }
    result
}
