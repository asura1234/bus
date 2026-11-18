use crate::{
    config::Config,
    error::{upstream, Error, Result},
};
use serde_json::Value;
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Control {
    pub stop: Arc<AtomicBool>,
    pub deadline: Option<Instant>,
}
impl Control {
    pub fn new(seconds: Option<u64>) -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            deadline: seconds.map(|n| Instant::now() + Duration::from_secs(n)),
        }
    }
    pub fn done(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.deadline.is_some_and(|d| Instant::now() >= d)
    }
}
pub fn command(path: &Path, args: &[String]) -> Command {
    let mut c = Command::new(path);
    c.args(args)
        .env("LARKSUITE_CLI_NO_UPDATE_NOTIFIER", "1")
        .env("LARKSUITE_CLI_NO_SKILLS_NOTIFIER", "1");
    // Every invoked command gets its own process group, including sink descendants.
    c.process_group(0);
    c
}
pub fn terminate(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        if child.try_wait().ok().flatten().is_some() || Instant::now() >= until {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}
fn nonblocking(fd: &impl std::os::fd::AsRawFd) -> Result<()> {
    let raw = fd.as_raw_fd();
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(upstream("cannot configure child pipe"));
    }
    Ok(())
}
pub fn run(
    path: &Path,
    args: &[String],
    input: Vec<u8>,
    timeout: u64,
    ctl: &Control,
    capture: bool,
) -> Result<(ExitStatus, Vec<u8>)> {
    let mut cmd = command(path, args);
    cmd.stdin(Stdio::piped())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|_| upstream("cannot launch child command"))?;
    // The timed loop owns both pipe ends. A detached descendant retaining a
    // pipe cannot strand an IO thread or prevent these handles being closed.
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take();
    let result = (|| {
        nonblocking(stdin.as_ref().unwrap())?;
        if let Some(pipe) = &stdout {
            nonblocking(pipe)?;
        }
        let until = Instant::now() + Duration::from_secs(timeout);
        let mut written = 0;
        let mut bytes = Vec::new();
        loop {
            if ctl.done() || Instant::now() >= until {
                return Err(upstream("child command timed out or stopped"));
            }
            if let Some(pipe) = stdin.as_mut() {
                while written < input.len() {
                    match pipe.write(&input[written..]) {
                        Ok(0) => return Err(upstream("child rejected stdin")),
                        Ok(n) => written += n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => return Err(upstream("child rejected stdin")),
                    }
                }
                if written == input.len() {
                    stdin.take();
                }
            }
            if let Some(pipe) = stdout.as_mut() {
                let mut buffer = [0; 8192];
                loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) => {
                            stdout.take();
                            break;
                        }
                        Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => return Err(upstream("cannot read child stdout")),
                    }
                    if bytes.len() > 16 * 1024 * 1024 {
                        return Err(upstream("child response exceeds limit"));
                    }
                }
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|_| upstream("cannot wait for child command"))?
            {
                if written != input.len() {
                    return Err(upstream("child did not consume input"));
                }
                // Drain again after observing exit: output could have arrived
                // between the earlier WouldBlock and try_wait. Never await EOF
                // retained by a detached descendant.
                if let Some(pipe) = stdout.as_mut() {
                    let mut buffer = [0; 8192];
                    loop {
                        match pipe.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => return Err(upstream("cannot read child stdout")),
                        }
                        if bytes.len() > 16 * 1024 * 1024 {
                            return Err(upstream("child response exceeds limit"));
                        }
                    }
                }
                return Ok((status, bytes));
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    drop(stdin);
    drop(stdout);
    terminate(&mut child);
    result
}
pub fn lark(
    c: &Config,
    args: &[String],
    input: Vec<u8>,
    ctl: &Control,
    envelope: bool,
) -> Result<Value> {
    let (status, stdout) = run(
        &c.lark_cli,
        args,
        input,
        c.command_timeout_seconds,
        ctl,
        true,
    )?;
    if !status.success() {
        return Err(upstream("lark-cli command failed"));
    }
    let v: Value =
        serde_json::from_slice(&stdout).map_err(|_| upstream("invalid lark-cli JSON response"))?;
    if envelope && v["ok"] != true {
        return Err(upstream("lark-cli returned an unsuccessful envelope"));
    }
    Ok(v)
}
pub fn verify(c: &Config, ctl: &Control) -> Result<()> {
    let v = lark(
        c,
        &["auth".into(), "status".into(), "--json".into()],
        vec![],
        ctl,
        false,
    )?;
    if v["appId"] != c.app_id
        || v["identities"]["bot"]["status"] != "ready"
        || v["identities"]["bot"]["available"] != true
    {
        return Err(upstream(
            "lark-cli app mismatch or bot identity unavailable",
        ));
    }
    Ok(())
}
pub fn api(c: &Config, path: &str, params: Value, ctl: &Control) -> Result<Value> {
    let v = lark(
        c,
        &[
            "api".into(),
            "GET".into(),
            path.into(),
            "--as".into(),
            "bot".into(),
            "--params".into(),
            params.to_string(),
            "--format".into(),
            "json".into(),
        ],
        vec![],
        ctl,
        true,
    )?;
    v.get("data")
        .cloned()
        .ok_or_else(|| upstream("missing lark-cli response data"))
}
pub fn send(
    c: &Config,
    open_id: Option<&str>,
    chat_id: Option<&str>,
    text: &str,
    key: Option<&str>,
    ctl: &Control,
) -> Result<Value> {
    verify(c, ctl)?;
    let mut args = vec![
        "im".into(),
        "+messages-send".into(),
        "--as".into(),
        "bot".into(),
    ];
    args.extend(if let Some(id) = open_id {
        vec!["--user-id".into(), id.into()]
    } else {
        vec!["--chat-id".into(), chat_id.unwrap().into()]
    });
    args.extend([
        "--text".into(),
        "-".into(),
        "--format".into(),
        "json".into(),
    ]);
    if let Some(k) = key {
        args.extend(["--idempotency-key".into(), k.into()]);
    }
    let v = lark(c, &args, text.as_bytes().to_vec(), ctl, true)?;
    let data = &v["data"];
    let id = data["message_id"]
        .as_str()
        .filter(|s| crate::config::valid_id(s, "om_"))
        .ok_or_else(|| upstream("send response missing message_id"))?;
    let chat = data["chat_id"]
        .as_str()
        .filter(|s| crate::config::valid_id(s, "oc_"))
        .ok_or_else(|| upstream("send response missing chat_id"))?;
    let store = crate::store::Store::open(c)
        .map_err(|_| Error(4, "state failed after send; send outcome uncertain".into()))?;
    store.context(&c.app_id, chat, id, "root")?;
    Ok(serde_json::json!({"message_id":id,"chat_id":chat}))
}
