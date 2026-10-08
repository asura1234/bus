use super::*;
use std::sync::Mutex;

fn env_lock() -> &'static Mutex<()> {
    crate::config::test_config_env_lock()
}

#[test]
fn configure_from_args_removes_global_session_option() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    let args = vec![
        "herdr".to_string(),
        "--session".to_string(),
        "work".to_string(),
        "workspace".to_string(),
        "list".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(std::env::var(SESSION_ENV_VAR).as_deref(), Ok("work"));
    assert!(explicit_session_requested());
    assert_eq!(cleaned, vec!["herdr", "workspace", "list"]);
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
}

#[test]
fn configure_from_args_accepts_equals_form() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    let args = vec![
        "herdr".to_string(),
        "server".to_string(),
        "stop".to_string(),
        "--session=api".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(std::env::var(SESSION_ENV_VAR).as_deref(), Ok("api"));
    assert!(explicit_session_requested());
    assert_eq!(cleaned, vec!["herdr", "server", "stop"]);
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
}

#[test]
fn configure_from_args_preserves_child_session_option_after_separator() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    let args = vec![
        "herdr".to_string(),
        "agent".to_string(),
        "start".to_string(),
        "repro".to_string(),
        "--".to_string(),
        "/bin/echo".to_string(),
        "--session".to_string(),
        "child-session".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(cleaned, args);
    assert!(std::env::var(SESSION_ENV_VAR).is_err());
    assert!(!explicit_session_requested());
}

#[test]
fn configure_from_args_preserves_child_session_equals_option_after_separator() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    let args = vec![
        "herdr".to_string(),
        "agent".to_string(),
        "start".to_string(),
        "repro".to_string(),
        "--".to_string(),
        "/bin/echo".to_string(),
        "--session=child-session".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(cleaned, args);
    assert!(std::env::var(SESSION_ENV_VAR).is_err());
    assert!(!explicit_session_requested());
}

#[test]
fn configure_from_args_rewrites_session_attach_to_default_launch() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var(SESSION_ENV_VAR, "bad/name");
    std::env::set_var(
        crate::utils::env::SOCKET_PATH_ENV_VAR,
        "/tmp/inherited.sock",
    );
    clear_explicit_session_for_test();
    let args = vec![
        "herdr".to_string(),
        "session".to_string(),
        "attach".to_string(),
        "work".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(std::env::var(SESSION_ENV_VAR).as_deref(), Ok("work"));
    assert!(explicit_session_requested());
    assert_eq!(cleaned, vec!["herdr"]);
    std::env::remove_var(SESSION_ENV_VAR);
    std::env::remove_var(crate::utils::env::SOCKET_PATH_ENV_VAR);
    clear_explicit_session_for_test();
}

#[test]
fn configure_from_args_leaves_session_attach_help_for_cli_dispatch() {
    let _guard = env_lock().lock().unwrap();
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    let args = vec![
        "herdr".to_string(),
        "session".to_string(),
        "attach".to_string(),
        "-h".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(cleaned, args);
    assert!(!explicit_session_requested());
}

#[test]
fn configure_from_args_maps_default_session_name_to_default_path() {
    let _guard = env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let config_home =
        std::env::temp_dir().join(format!("herdr-session-default-{}", std::process::id()));
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::set_var(SESSION_ENV_VAR, "work");
    clear_explicit_session_for_test();
    std::env::set_var(
        crate::utils::env::SOCKET_PATH_ENV_VAR,
        "/tmp/inherited.sock",
    );
    let args = vec![
        "herdr".to_string(),
        "--session".to_string(),
        DEFAULT_SESSION_NAME.to_string(),
        "workspace".to_string(),
        "list".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(cleaned, vec!["herdr", "workspace", "list"]);
    assert!(std::env::var(SESSION_ENV_VAR).is_err());
    assert!(explicit_session_requested());
    assert_eq!(
        active_api_socket_path(),
        config_home
            .join(crate::config::app_dir_name())
            .join("herdr.sock")
    );
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    std::env::remove_var(crate::utils::env::SOCKET_PATH_ENV_VAR);
}

#[test]
fn env_session_does_not_mark_session_explicit() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var(SESSION_ENV_VAR, "env-session");
    EXPLICIT_SESSION_REQUESTED.store(true, Ordering::Relaxed);
    let args = vec![
        "herdr".to_string(),
        "workspace".to_string(),
        "list".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(cleaned, vec!["herdr", "workspace", "list"]);
    assert_eq!(std::env::var(SESSION_ENV_VAR).as_deref(), Ok("env-session"));
    assert!(!explicit_session_requested());
    std::env::remove_var(SESSION_ENV_VAR);
}

#[test]
fn env_default_session_name_uses_default_path() {
    let _guard = env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let config_home =
        std::env::temp_dir().join(format!("herdr-env-session-default-{}", std::process::id()));
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::remove_var(crate::utils::env::SOCKET_PATH_ENV_VAR);
    std::env::set_var(SESSION_ENV_VAR, DEFAULT_SESSION_NAME);
    EXPLICIT_SESSION_REQUESTED.store(true, Ordering::Relaxed);
    let args = vec![
        "herdr".to_string(),
        "workspace".to_string(),
        "list".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(cleaned, vec!["herdr", "workspace", "list"]);
    assert!(std::env::var(SESSION_ENV_VAR).is_err());
    assert!(!explicit_session_requested());
    assert_eq!(
        active_api_socket_path(),
        config_home
            .join(crate::config::app_dir_name())
            .join("herdr.sock")
    );
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var(SESSION_ENV_VAR);
    std::env::remove_var(crate::utils::env::SOCKET_PATH_ENV_VAR);
    clear_explicit_session_for_test();
}

#[test]
fn explicit_session_socket_ignores_inherited_socket_override() {
    let _guard = env_lock().lock().unwrap();
    let _bus = crate::config::test_without_bus_env(&_guard);
    let config_home =
        std::env::temp_dir().join(format!("herdr-session-precedence-{}", std::process::id()));
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::set_var(SESSION_ENV_VAR, "work");
    EXPLICIT_SESSION_REQUESTED.store(true, Ordering::Relaxed);
    std::env::set_var(
        crate::utils::env::SOCKET_PATH_ENV_VAR,
        "/tmp/inherited.sock",
    );

    let path = active_api_socket_path();

    assert_eq!(
        path,
        config_home
            .join(crate::config::app_dir_name())
            .join("sessions")
            .join("work")
            .join("herdr.sock")
    );
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    std::env::remove_var(crate::utils::env::SOCKET_PATH_ENV_VAR);
}

#[test]
fn env_socket_override_wins_without_explicit_session() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var(SESSION_ENV_VAR, "work");
    clear_explicit_session_for_test();
    std::env::set_var(crate::utils::env::SOCKET_PATH_ENV_VAR, "/tmp/explicit.sock");

    assert_eq!(
        active_api_socket_path(),
        PathBuf::from("/tmp/explicit.sock")
    );

    std::env::set_var(crate::utils::env::SOCKET_PATH_ENV_VAR, "");
    assert_eq!(active_api_socket_path(), PathBuf::new());

    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    std::env::remove_var(crate::utils::env::SOCKET_PATH_ENV_VAR);
}

#[test]
fn env_socket_override_skips_invalid_env_session_validation_without_explicit_session() {
    let _guard = env_lock().lock().unwrap();
    std::env::set_var(SESSION_ENV_VAR, "bad/name");
    clear_explicit_session_for_test();
    std::env::set_var(crate::utils::env::SOCKET_PATH_ENV_VAR, "/tmp/herdr.sock");
    let args = vec![
        "herdr".to_string(),
        "workspace".to_string(),
        "list".to_string(),
    ];

    let cleaned = configure_from_args(&args).unwrap();

    assert_eq!(cleaned, vec!["herdr", "workspace", "list"]);
    assert!(!explicit_session_requested());
    assert_eq!(active_api_socket_path(), PathBuf::from("/tmp/herdr.sock"));
    assert_eq!(std::env::var(SESSION_ENV_VAR).as_deref(), Ok("bad/name"));

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = std::ffi::OsString::from_vec(b"/tmp/herdr-\xff.sock".to_vec());
        std::env::set_var(crate::utils::env::SOCKET_PATH_ENV_VAR, path);
        // Presence bypasses inherited session validation even when var() cannot
        // read the socket path; path selection then falls back to the default.
        assert_eq!(configure_from_args(&args).unwrap(), cleaned);
        assert_eq!(active_api_socket_path(), api_socket_path_for(None));
        assert_eq!(std::env::var(SESSION_ENV_VAR).as_deref(), Ok("bad/name"));
    }

    std::env::remove_var(SESSION_ENV_VAR);
    clear_explicit_session_for_test();
    std::env::remove_var(crate::utils::env::SOCKET_PATH_ENV_VAR);
}

#[test]
fn invalid_names_are_rejected() {
    let _guard = env_lock().lock().unwrap();
    assert!(validate_name("../prod").is_err());
    assert!(validate_name("").is_err());
    assert!(validate_name("work session").is_err());
}
