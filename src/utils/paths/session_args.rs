use super::{validate_name, DEFAULT_SESSION_NAME, EXPLICIT_SESSION_REQUESTED, SESSION_ENV_VAR};
use std::sync::atomic::Ordering;

pub fn configure_from_args(args: &[String]) -> Result<Vec<String>, String> {
    let mut cleaned = Vec::with_capacity(args.len());
    if let Some(program) = args.first() {
        cleaned.push(program.clone());
    }

    if args.get(1).map(String::as_str) == Some("session")
        && args.get(2).map(String::as_str) == Some("attach")
    {
        if matches!(
            args.get(3).map(String::as_str),
            Some("help" | "--help" | "-h")
        ) {
            return Ok(args.to_vec());
        }
        let Some(name) = args.get(3) else {
            return Err("usage: herdr session attach <name>".to_string());
        };
        if args.len() != 4 {
            return Err("usage: herdr session attach <name>".to_string());
        }
        apply_explicit_name(name)?;
        return Ok(cleaned);
    }

    let mut requested_session = None;
    let mut index = 1;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            cleaned.extend_from_slice(&args[index..]);
            break;
        }
        if arg == "--session" {
            let Some(value) = args.get(index + 1) else {
                return Err("missing value for --session".to_string());
            };
            requested_session = Some(value.clone());
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--session=") {
            requested_session = Some(value.to_string());
            index += 1;
            continue;
        }

        cleaned.push(arg.clone());
        index += 1;
    }

    if let Some(session) = requested_session {
        apply_explicit_name(&session)?;
    } else if std::env::var_os(crate::utils::env::SOCKET_PATH_ENV_VAR).is_some() {
        EXPLICIT_SESSION_REQUESTED.store(false, Ordering::Relaxed);
    } else if let Ok(session) = std::env::var(SESSION_ENV_VAR) {
        if normalize_name(&session)?.is_none() {
            std::env::remove_var(SESSION_ENV_VAR);
        }
        EXPLICIT_SESSION_REQUESTED.store(false, Ordering::Relaxed);
    } else {
        EXPLICIT_SESSION_REQUESTED.store(false, Ordering::Relaxed);
    }

    Ok(cleaned)
}

pub(super) fn apply_explicit_name(name: &str) -> Result<(), String> {
    let session = normalize_name(name)?;
    if let Some(session) = session {
        std::env::set_var(SESSION_ENV_VAR, session);
    } else {
        std::env::remove_var(SESSION_ENV_VAR);
    }
    EXPLICIT_SESSION_REQUESTED.store(true, Ordering::Relaxed);
    Ok(())
}

fn normalize_name(name: &str) -> Result<Option<String>, String> {
    if name == DEFAULT_SESSION_NAME {
        return Ok(None);
    }
    validate_name(name)?;
    Ok(Some(name.to_string()))
}
