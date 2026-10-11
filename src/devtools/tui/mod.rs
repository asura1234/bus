//! `bus --dev tui`: drive a fresh scratch Bus through its real terminal.
//!
//! Every session is a new Bus started by the driver itself, in a generated data
//! directory with its own server and hidden terminal. No verb attaches to an
//! existing session. See `docs/tui-driver.md` for the command reference, which
//! is also this command's `--help`.
mod args;
mod fake_agents;
mod host;
mod input;
mod predicate;
mod protocol;
mod screen;
mod session;
mod trace;

pub(crate) use fake_agents::{fake_provider, run as run_fake_agent};

use protocol::{exit_code, failure, Command, EXIT_NO_SESSION, EXIT_OK, EXIT_USAGE};
use serde_json::{json, Value};
use session::{SessionPaths, DEFAULT_NAME};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

const HELP: &str = include_str!("../../../docs/tui-driver.md");
const HOST_START_TIMEOUT: Duration = Duration::from_secs(60);
/// Selects the session when a verb has no `--name`.
const SESSION_ENV: &str = "BUS_TUI_SESSION";

/// Entry of `bus --dev tui ARGS`; returns the process exit code.
pub(crate) fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("--help" | "-h" | "help") => {
            write_out(&args::help_overview(HELP));
            EXIT_OK
        }
        Some(verb) if args.iter().skip(1).any(|a| a == "--help" || a == "-h") => {
            match args::help_for(HELP, verb) {
                Some(text) => {
                    write_out(&text);
                    EXIT_OK
                }
                None => usage(format!("unknown verb {verb:?}; run `bus --dev tui --help`")),
            }
        }
        Some("__host") => run_host(&args[1..]),
        Some("snapshot") if args.iter().any(|a| a == "--plain") => {
            let rest: Vec<String> = args[1..]
                .iter()
                .filter(|a| *a != "--plain")
                .cloned()
                .collect();
            let response = dispatch("snapshot", &rest);
            if response["ok"].as_bool() != Some(true) {
                return emit(&response);
            }
            for row in response["rows"].as_array().into_iter().flatten() {
                write_line(row.as_str().unwrap_or(""));
            }
            EXIT_OK
        }
        Some(verb) => {
            let response = dispatch(verb, &args[1..]);
            emit(&response)
        }
    }
}

/// Command output goes through one locked writer; Bus lints forbid print macros.
fn write_out(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

fn write_line(text: &str) {
    write_out(&format!("{text}\n"));
}

fn usage(message: String) -> i32 {
    emit(&failure("usage", message))
}

/// Prints one JSON line on stdout; failures also get a one-line reason on stderr.
fn emit(response: &Value) -> i32 {
    write_line(&response.to_string());
    let code = exit_code(response);
    if code != EXIT_OK {
        let error = &response["error"];
        eprintln!(
            "error: {} {}",
            error["code"].as_str().unwrap_or("failed"),
            error["message"].as_str().unwrap_or("")
        );
    }
    code
}

fn dispatch(verb: &str, rest: &[String]) -> Value {
    if verb == "bus" {
        // Everything after the verb belongs to Bus, except a leading --name.
        let name = match rest {
            [flag, name, ..] if flag == "--name" => name.clone(),
            _ => std::env::var(SESSION_ENV).unwrap_or_else(|_| DEFAULT_NAME.to_owned()),
        };
        if let Err(message) = session::validate_name(&name) {
            return failure("usage", message);
        }
        return passthrough(&name, rest_without_name(rest));
    }
    let mut parsed = match args::Parsed::parse(rest) {
        Ok(parsed) => parsed,
        Err(message) => return failure("usage", message),
    };
    let name = parsed
        .take_value("--name")
        .or_else(|| std::env::var(SESSION_ENV).ok())
        .unwrap_or_else(|| DEFAULT_NAME.to_owned());
    if let Err(message) = session::validate_name(&name) {
        return failure("usage", message);
    }
    let result = match verb {
        "start" => start(&name, &mut parsed),
        "stop" => parsed.finish().map(|()| stop(&name)),
        "list" => parsed.finish().map(|()| list()),
        "wait" | "expect" => wait(&name, &mut parsed),
        "journey" => journey(&name, &mut parsed),
        "gc" => gc(&mut parsed),
        _ => args::command(verb, &mut parsed).map(|command| request(&name, &command)),
    };
    result.unwrap_or_else(|message| failure("usage", message))
}

/// `tui bus` forwards everything verbatim except a leading `--name NAME`.
fn rest_without_name(rest: &[String]) -> Vec<String> {
    match rest {
        [flag, _, tail @ ..] if flag == "--name" => tail.to_vec(),
        _ => rest.to_vec(),
    }
}

fn paths(name: &str) -> SessionPaths {
    SessionPaths::new(&session::driver_root(), name)
}

/// Sends one request to the session's host and returns its JSON reply.
fn request(name: &str, command: &Command) -> Value {
    let paths = paths(name);
    if !session::session_alive(&paths) {
        return failure(
            "no_session",
            format!("no running driver session {name:?}; start one with `bus --dev tui start --name {name}`"),
        );
    }
    let stream = match crate::platform::ipc::connect_local_stream(&paths.socket()) {
        Ok(stream) => stream,
        Err(error) => return failure("host_unreachable", format!("session {name:?}: {error}")),
    };
    let mut reader = BufReader::new(stream);
    let line = match serde_json::to_string(command) {
        Ok(line) => line,
        Err(error) => return failure("usage", error.to_string()),
    };
    if writeln!(reader.get_mut(), "{line}")
        .and_then(|()| reader.get_mut().flush())
        .is_err()
    {
        return failure(
            "host_unreachable",
            format!("session {name:?} closed the connection"),
        );
    }
    let mut reply = String::new();
    match reader.read_line(&mut reply) {
        Ok(n) if n > 0 => serde_json::from_str(&reply)
            .unwrap_or_else(|_| failure("host_unreachable", "unreadable reply from host")),
        _ => failure(
            "host_unreachable",
            format!("session {name:?} sent no reply"),
        ),
    }
}

fn start(name: &str, parsed: &mut args::Parsed) -> Result<Value, String> {
    let (cols, rows) = match parsed.take_value("--size") {
        Some(size) => args::parse_size(&size)?,
        None => (160, 45),
    };
    let run_root = parsed.take_value("--run-dir").map(PathBuf::from);
    parsed.finish()?;
    let started = Instant::now();
    let binary = std::env::current_exe().map_err(|e| e.to_string())?;
    let allow_target_debug = std::env::var_os(session::ALLOW_TARGET_DEBUG_ENV).is_some();
    if let Err(message) = session::ensure_binary_allowed(&binary, allow_target_debug) {
        return Ok(failure("refused", message));
    }
    let root = session::driver_root();
    session::create_private_dir(&root).map_err(|e| e.to_string())?;
    let paths = SessionPaths::new(&root, name);
    if let Err(refusal) = prepare_target(&root, &paths, &session::inherited_env()) {
        return Ok(refusal);
    }
    paths.create().map_err(|e| e.to_string())?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| root.clone());
    let run_dir = run_root
        .unwrap_or_else(|| session::default_runs_root(&cwd))
        .join(format!("{name}-{}", timestamp()));

    let log = std::fs::File::create(paths.host_log()).map_err(|e| e.to_string())?;
    let mut host = std::process::Command::new(&binary);
    host.args(["--dev", "tui", "__host", "--name", name])
        .args(["--size", &format!("{cols}x{rows}")])
        .arg("--run-dir")
        .arg(&run_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    crate::platform::detach_server_daemon_command(&mut host);
    let mut child = host
        .spawn()
        .map_err(|e| format!("cannot start host: {e}"))?;

    let deadline = Instant::now() + HOST_START_TIMEOUT;
    loop {
        if paths.socket().exists() && session::session_alive(&paths) {
            let mut status = request(name, &Command::Status);
            status["ready_ms"] = json!(started.elapsed().as_millis() as u64);
            return Ok(status);
        }
        let error = std::fs::read_to_string(paths.dir.join("host.error")).ok();
        let exited = child.try_wait().ok().flatten().is_some();
        if error.is_some() || exited || Instant::now() > deadline {
            let _ = child.kill();
            let detail = error.unwrap_or_else(|| {
                std::fs::read_to_string(paths.host_log())
                    .map(|log| log.lines().rev().take(20).collect::<Vec<_>>().join("\n"))
                    .unwrap_or_default()
            });
            let _ = paths.remove();
            return Ok(failure(
                "start_failed",
                format!("session {name:?} did not start: {detail}"),
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Every guard that keeps a new session away from live Bus instances.
fn prepare_target(
    root: &Path,
    paths: &SessionPaths,
    inherited: &[(String, OsString)],
) -> Result<(), Value> {
    let live = session::live_sessions(root);
    if live.iter().any(|info| info.name == paths.name) {
        return Err(failure(
            "refused",
            format!(
                "session {:?} is already running; stop it or pick another --name",
                paths.name
            ),
        ));
    }
    if live.len() >= session::MAX_SESSIONS {
        return Err(failure(
            "refused",
            format!(
                "{} driver sessions are running ({}); stop one first",
                live.len(),
                live.iter()
                    .map(|i| i.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    session::ensure_env_does_not_point_at(&paths.dir, inherited)
        .and_then(|()| session::ensure_outside_bus_homes(&paths.dir, &session::bus_homes()))
        .and_then(|()| session::ensure_not_live_bus(&paths.data_dir()))
        .map_err(|message| failure("refused", message))?;
    if paths.dir.exists() {
        if !paths.is_driver_dir() {
            return Err(failure(
                "refused",
                format!(
                    "{} exists and was not created by the driver; remove it yourself",
                    paths.dir.display()
                ),
            ));
        }
        paths
            .remove()
            .map_err(|e| failure("refused", format!("cannot clear stale session: {e}")))?;
    }
    Ok(())
}

fn stop(name: &str) -> Value {
    let paths = paths(name);
    let info = paths.read_info();
    let response = request(name, &Command::Stop);
    if response["ok"].as_bool() == Some(true) {
        if let Some(info) = info {
            let deadline = Instant::now() + Duration::from_secs(10);
            while session::process_alive(info.host_pid) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    response
}

fn list() -> Value {
    let sessions: Vec<Value> = session::live_sessions(&session::driver_root())
        .into_iter()
        .map(|info| json!({"session": info.name, "size": info.size, "run_dir": info.run_dir}))
        .collect();
    json!({"ok": true, "sessions": sessions, "max": session::MAX_SESSIONS})
}

pub(super) struct CommandOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
}

/// Runs `bus ARGS` against a session with exactly `env` and a time limit.
pub(super) fn bus_command(
    binary: &Path,
    env: &[(String, OsString)],
    args: &[&str],
    timeout: Duration,
) -> CommandOutput {
    let mut command = std::process::Command::new(binary);
    command
        .args(args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Ok(mut child) = command.spawn() else {
        return CommandOutput {
            status: None,
            stdout: String::new(),
            stderr: "cannot run bus".into(),
        };
    };
    let deadline = Instant::now() + timeout;
    while child.try_wait().ok().flatten().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    match child.wait_with_output() {
        Ok(output) => CommandOutput {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Err(error) => CommandOutput {
            status: None,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

fn session_env(name: &str) -> Result<(PathBuf, Vec<(String, OsString)>), Value> {
    let paths = paths(name);
    let info = paths
        .read_info()
        .filter(|info| session::process_alive(info.host_pid))
        .ok_or_else(|| failure("no_session", format!("no running driver session {name:?}")))?;
    Ok((
        info.binary,
        session::scratch_env(&paths, &session::inherited_env()),
    ))
}

/// `tui bus ARGS`: a Bus command against the session's scratch Bus only.
fn passthrough(name: &str, args: Vec<String>) -> Value {
    if args.is_empty() || args.first().is_some_and(|a| a == "tui" || a == "--dev") {
        return failure(
            "usage",
            "usage: bus --dev tui bus <command> [args], e.g. `tui bus state`",
        );
    }
    let (binary, env) = match session_env(name) {
        Ok(found) => found,
        Err(error) => return error,
    };
    let status = std::process::Command::new(binary)
        .args(&args)
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .status();
    let code = status
        .ok()
        .and_then(|s| s.code())
        .unwrap_or(EXIT_NO_SESSION);
    // The command already printed its own output.
    std::process::exit(code);
}

fn wait(name: &str, parsed: &mut args::Parsed) -> Result<Value, String> {
    let timeout = parsed
        .take_value("--timeout")
        .map(|t| args::parse_duration(&t))
        .transpose()?
        .unwrap_or(Duration::from_secs(10));
    if let Some(predicate) = parsed.take_value("--state") {
        parsed.finish()?;
        let predicate = predicate::Predicate::parse(&predicate)?;
        return Ok(wait_state(name, &predicate, timeout));
    }
    if let Some(message) = parsed.take_value("--message") {
        parsed.finish()?;
        let (binary, env) = match session_env(name) {
            Ok(found) => found,
            Err(error) => return Ok(error),
        };
        let secs = timeout.as_secs().clamp(1, 600).to_string();
        let output = bus_command(
            &binary,
            &env,
            &["wait", "--message", &message, "--timeout", &secs],
            timeout + Duration::from_secs(10),
        );
        let reply: Value = output
            .stdout
            .lines()
            .last()
            .and_then(|line| serde_json::from_str(line).ok())
            .unwrap_or(Value::Null);
        return Ok(if output.ok() {
            json!({"ok": true, "message": reply})
        } else {
            let mut response = failure("timeout", format!("message {message} did not settle"));
            response["message"] = reply;
            response
        });
    }
    let command = args::wait_command(parsed, timeout)?;
    Ok(request(name, &command))
}

fn wait_state(name: &str, predicate: &predicate::Predicate, timeout: Duration) -> Value {
    let (binary, env) = match session_env(name) {
        Ok(found) => found,
        Err(error) => return error,
    };
    let started = Instant::now();
    loop {
        let output = bus_command(&binary, &env, &["state"], Duration::from_secs(20));
        let state: Value = serde_json::from_str(output.stdout.trim()).unwrap_or(Value::Null);
        let result = &state["result"];
        if predicate.holds(result) {
            return json!({"ok": true, "waited_ms": started.elapsed().as_millis() as u64});
        }
        if started.elapsed() >= timeout {
            let mut response = failure(
                "timeout",
                format!(
                    "state never matched {} within {} ms",
                    predicate.source,
                    timeout.as_millis()
                ),
            );
            response["actual"] = predicate.actual(result);
            return response;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Runs steps from a JSON file in one session; stops at the first failure.
fn journey(name: &str, parsed: &mut args::Parsed) -> Result<Value, String> {
    let file = parsed
        .positional()
        .ok_or("usage: bus --dev tui journey STEPS.json")?;
    parsed.finish()?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?;
    let steps: Vec<Value> = serde_json::from_str(&text).map_err(|e| format!("{file}: {e}"))?;
    for (index, step) in steps.iter().enumerate() {
        let verb = step["verb"]
            .as_str()
            .ok_or(format!("step {index}: missing verb"))?;
        if matches!(verb, "start" | "stop" | "journey" | "bus") {
            return Err(format!("step {index}: {verb} is not allowed in a journey"));
        }
        let mut args: Vec<String> = step["args"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        args.extend(["--name".into(), name.to_owned()]);
        let response = dispatch(verb, &args);
        write_line(&json!({"step": index, "verb": verb, "result": response}).to_string());
        if exit_code(&response) != EXIT_OK {
            let mut failed = failure(
                response["error"]["code"].as_str().unwrap_or("failed"),
                format!("journey stopped at step {index} ({verb})"),
            );
            failed["steps_passed"] = json!(index);
            return Ok(failed);
        }
        if let Some(pause) = step["pause_ms"].as_u64() {
            std::thread::sleep(Duration::from_millis(pause));
        }
    }
    Ok(json!({"ok": true, "steps_passed": steps.len()}))
}

/// Deletes run folders older than `--older-than` (default 7d), keeping live ones.
fn gc(parsed: &mut args::Parsed) -> Result<Value, String> {
    let age = parsed
        .take_value("--older-than")
        .map(|v| args::parse_duration(&v))
        .transpose()?
        .unwrap_or(Duration::from_secs(7 * 24 * 3600));
    let root = parsed
        .take_value("--run-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            session::default_runs_root(&std::env::current_dir().unwrap_or_default())
        });
    parsed.finish()?;
    let live: Vec<PathBuf> = session::live_sessions(&session::driver_root())
        .into_iter()
        .map(|info| info.run_dir)
        .collect();
    let mut removed = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|elapsed| elapsed > age);
            if path.is_dir()
                && old
                && !live.contains(&path)
                && path.join("pty.cast").exists()
                && std::fs::remove_dir_all(&path).is_ok()
            {
                removed.push(path);
            }
        }
    }
    Ok(json!({"ok": true, "removed": removed}))
}

fn run_host(args: &[String]) -> i32 {
    let mut parsed = match args::Parsed::parse(args) {
        Ok(parsed) => parsed,
        Err(message) => return usage(message),
    };
    let options = (|| -> Result<host::HostOptions, String> {
        let name = parsed.take_value("--name").ok_or("missing --name")?;
        session::validate_name(&name)?;
        let (cols, rows) = args::parse_size(&parsed.take_value("--size").ok_or("missing --size")?)?;
        let run_dir = PathBuf::from(parsed.take_value("--run-dir").ok_or("missing --run-dir")?);
        parsed.finish()?;
        Ok(host::HostOptions {
            name,
            cols,
            rows,
            run_dir,
        })
    })();
    match options {
        Ok(options) => host::run(&session::driver_root(), options),
        Err(message) => {
            eprintln!("bus tui host: {message}");
            EXIT_USAGE
        }
    }
}

fn timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}
