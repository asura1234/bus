use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct MatchedRule {
    pub(super) id: String,
}

#[derive(Debug)]
pub(super) struct Detected {
    pub(super) detection: AgentDetection,
    pub(super) matched_rule: Option<MatchedRule>,
}

impl std::ops::Deref for Detected {
    type Target = AgentDetection;

    fn deref(&self) -> &Self::Target {
        &self.detection
    }
}

pub(super) fn detect_input(agent: AgentKind, input: DetectionInput<'_>) -> Detected {
    let detection = detect_with_osc(agent, input);
    let matched_rule = load_manifest(agent).and_then(|loaded| {
        matched_manifest_rule(input, &loaded).map(|rule| MatchedRule {
            id: rule.id.clone(),
        })
    });
    Detected {
        detection,
        matched_rule,
    }
}

pub(super) fn detect_loaded(input: DetectionInput<'_>, loaded: &LoadedManifest) -> Detected {
    Detected {
        detection: evaluate_loaded_manifest(input, loaded),
        matched_rule: matched_manifest_rule(input, loaded).map(|rule| MatchedRule {
            id: rule.id.clone(),
        }),
    }
}

pub(super) fn detect_screen(agent: AgentKind, screen: &str) -> Detected {
    detect_input(
        agent,
        DetectionInput {
            screen,
            osc_title: "",
            osc_progress: "",
        },
    )
}

pub(super) fn local_manifest(state: &str, contains: &str) -> String {
    format!(
        r#"
id = "codex"

[[rules]]
id = "test"
state = "{state}"
contains = ["{contains}"]
"#
    )
}

pub(super) fn rules_manifest(rules: &str) -> String {
    format!(
        r#"
id = "codex"

{rules}
"#
    )
}

pub(super) fn with_manifest_dirs<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let bus = crate::utils::config::test_without_bus_env(&_guard);
    let old_config = std::env::var_os("XDG_CONFIG_HOME");
    let old_state = std::env::var_os("XDG_STATE_HOME");
    let base =
        std::env::temp_dir().join(format!("bus-manifest-loader-{name}-{}", std::process::id()));
    let config_dir = base.join("config");
    let state_dir = base.join("state");
    let _ = std::fs::remove_dir_all(&base);
    std::env::set_var("XDG_CONFIG_HOME", &config_dir);
    std::env::set_var("XDG_STATE_HOME", &state_dir);
    reload_manifests();
    let result = f();
    match old_config {
        Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
    match old_state {
        Some(value) => std::env::set_var("XDG_STATE_HOME", value),
        None => std::env::remove_var("XDG_STATE_HOME"),
    }
    drop(bus);
    reload_manifests();
    let _ = std::fs::remove_dir_all(&base);
    result
}

pub(super) fn write_local_codex_without_reload(content: &str) {
    let path = override_path(AgentKind::Codex).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

pub(super) fn write_local_codex(content: &str) {
    write_local_codex_without_reload(content);
    reload_manifests();
}

pub(super) fn detect_screen_with_osc(
    agent: AgentKind,
    screen: &str,
    osc_title: &str,
    osc_progress: &str,
) -> Detected {
    detect_input(
        agent,
        DetectionInput {
            screen,
            osc_title,
            osc_progress,
        },
    )
}

// --- Claude OSC rules ---

#[test]
fn known_agent_no_match_defaults_to_idle_fallback() {
    let result = detect_screen(AgentKind::Codex, "ordinary prompt text");

    assert_eq!(result.state, AgentState::Idle);
    assert!(!result.visible_idle);
}

#[test]
fn rule_semantics_apply_gates_priority_and_line_regex() {
    with_manifest_dirs("rule-semantics", || {
        write_local_codex(&rules_manifest(
            r#"
[[rules]]
id = "low_contains"
state = "idle"
priority = 1
contains = ["match"]

[[rules]]
id = "high_nested_gates"
state = "working"
priority = 10
contains = ["match"]
all = [
  { any = [{ regex = ["w[io]n"] }, { contains = ["fallback"] }] },
]
not = [
  { contains = ["blocked"] },
]

[[rules]]
id = "line_regex"
state = "blocked"
priority = 20
line_regex = ["^exact line$"]
"#,
        ));

        let high = detect_screen(AgentKind::Codex, "match win");
        assert_eq!(high.state, AgentState::Working);
        assert_eq!(
            high.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("high_nested_gates")
        );

        let not_gate = detect_screen(AgentKind::Codex, "match win blocked");
        assert_eq!(not_gate.state, AgentState::Idle);
        assert_eq!(
            not_gate.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("low_contains")
        );

        let line = detect_screen(AgentKind::Codex, "before\nexact line\nafter");
        assert_eq!(line.state, AgentState::Blocked);
        assert_eq!(
            line.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("line_regex")
        );
    });
}

#[test]
fn equal_priority_rules_keep_the_first_matching_rule() {
    let manifest = parse_manifest(&rules_manifest(
        r#"
[[rules]]
id = "first"
state = "working"
priority = 10
visible_working = true
contains = ["match"]

[[rules]]
id = "second"
state = "blocked"
priority = 10
visible_blocker = true
contains = ["match"]
"#,
    ))
    .unwrap();
    let loaded = loaded_manifest(manifest).unwrap();
    let result = detect_loaded(
        DetectionInput {
            screen: "match",
            osc_title: "",
            osc_progress: "",
        },
        &loaded,
    );

    assert_eq!(result.state, AgentState::Working);
    assert!(result.visible_working);
    assert!(!result.visible_blocker);
    assert_eq!(result.matched_rule.unwrap().id, "first");
}

#[test]
fn local_override_replaces_bundled_manifest() {
    with_manifest_dirs("local-source", || {
        write_local_codex(&local_manifest("blocked", "local-ready"));

        let result = detect_screen(AgentKind::Codex, "local-ready");

        assert_eq!(result.state, AgentState::Blocked);
    });
}

#[test]
fn invalid_local_override_falls_back_to_bundled_manifest() {
    with_manifest_dirs("invalid-local-bundled-fallback", || {
        write_local_codex("id = ");
        let result = detect_input(
            AgentKind::Codex,
            DetectionInput {
                screen: "",
                osc_title: "⠋ project",
                osc_progress: "",
            },
        );
        assert_eq!(result.state, AgentState::Working);
        assert!(result.visible_working);
        assert_eq!(result.matched_rule.unwrap().id, "osc_title_working");
    });
}

#[test]
fn detection_uses_cached_manifest_until_explicit_reload() {
    with_manifest_dirs("cache-boundary", || {
        write_local_codex(&local_manifest("blocked", "cached-ready"));

        let cached = detect_screen(AgentKind::Codex, "cached-ready");
        assert_eq!(cached.state, AgentState::Blocked);
        assert_eq!(
            cached.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("test")
        );

        write_local_codex_without_reload(&local_manifest("working", "new-ready"));

        let unchanged = detect_screen(AgentKind::Codex, "new-ready");
        assert_eq!(unchanged.state, AgentState::Idle);

        reload_manifests();

        let reloaded = detect_screen(AgentKind::Codex, "new-ready");
        assert_eq!(reloaded.state, AgentState::Working);
        assert_eq!(
            reloaded.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("test")
        );
    });
}
