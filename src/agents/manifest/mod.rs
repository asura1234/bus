mod compile;
mod loader;
mod regions;
mod schema;

use self::compile::compiled_rule_matches;
#[cfg(test)]
pub(crate) use self::compile::parse_manifest;
#[cfg(test)]
pub(crate) use self::loader::reload_manifests;
#[cfg(test)]
use self::loader::{bundled_loaded_manifest, bundled_manifest, loaded_manifest, override_path};
use self::loader::{load_manifest, LoadedManifest};
use self::regions::region;
use self::schema::ManifestRule;
use super::{agent_label, parse_agent_label, AgentDetection, AgentKind, AgentState};

/// Input to the detection engine, carrying the screen snapshot plus any
/// OSC-derived strings captured from the terminal title / progress sequences.
/// Pass empty strings for `osc_title` and `osc_progress` when the data is not
/// available — behavior is identical to the pre-OSC engine in that case.
#[derive(Debug, Clone, Copy)]
pub struct DetectionInput<'a> {
    pub screen: &'a str,
    pub osc_title: &'a str,
    pub osc_progress: &'a str,
}

pub fn detect_with_osc(agent: AgentKind, input: DetectionInput<'_>) -> AgentDetection {
    detect_with_launch_context(agent, input, false)
}

pub(crate) fn detect_with_launch_context(
    agent: AgentKind,
    input: DetectionInput<'_>,
    managed_launch_pending: bool,
) -> AgentDetection {
    let Some(loaded) = load_manifest(agent) else {
        return fallback_detection(managed_launch_pending);
    };
    evaluate_loaded_manifest(input, &loaded, managed_launch_pending)
}

fn evaluate_loaded_manifest(
    input: DetectionInput<'_>,
    loaded: &LoadedManifest,
    managed_launch_pending: bool,
) -> AgentDetection {
    evaluate_loaded_manifest_skipping(input, loaded, managed_launch_pending, None)
}

fn evaluate_loaded_manifest_skipping(
    input: DetectionInput<'_>,
    loaded: &LoadedManifest,
    managed_launch_pending: bool,
    skip_rules: Option<&[&str]>,
) -> AgentDetection {
    let Some(rule) = matched_manifest_rule(input, loaded, managed_launch_pending, skip_rules)
    else {
        return fallback_detection(managed_launch_pending);
    };
    let state = rule
        .state
        .map(AgentState::from)
        .unwrap_or(AgentState::Unknown);
    AgentDetection {
        state,
        skip_state_update: rule.skip_state_update,
        visible_idle: rule.visible_idle && state == AgentState::Idle,
        visible_blocker: rule.visible_blocker && state == AgentState::Blocked,
        visible_working: rule.visible_working && state == AgentState::Working,
    }
}

fn kimi_screen_input(screen: &str) -> DetectionInput<'_> {
    DetectionInput {
        screen,
        osc_title: "",
        osc_progress: "",
    }
}

/// True when the running-command line, not a higher-priority Kimi marker, won.
pub(crate) fn kimi_running_command_is_winner(screen: &str) -> bool {
    let Some(loaded) = load_manifest(AgentKind::Kimi) else {
        return false;
    };
    matched_manifest_rule(kimi_screen_input(screen), &loaded, false, None)
        .is_some_and(|rule| rule.id == "footer_running_command_working")
}

/// Screen state after refusing a command label and its accompanying tip as proof of work.
pub(crate) fn kimi_screen_without_running_command(screen: &str) -> AgentDetection {
    let Some(loaded) = load_manifest(AgentKind::Kimi) else {
        return fallback_detection(false);
    };
    evaluate_loaded_manifest_skipping(
        kimi_screen_input(screen),
        &loaded,
        false,
        Some(&["footer_running_command_working", "moon_tip_spinner_working"]),
    )
}

pub fn should_skip_state_update(agent: AgentKind, screen_content: &str) -> bool {
    detect_with_osc(
        agent,
        DetectionInput {
            screen: screen_content,
            osc_title: "",
            osc_progress: "",
        },
    )
    .skip_state_update
}

fn matched_manifest_rule<'a>(
    input: DetectionInput<'_>,
    loaded: &'a LoadedManifest,
    managed_launch_pending: bool,
    skip_rules: Option<&[&str]>,
) -> Option<&'a ManifestRule> {
    let mut matched: Option<&ManifestRule> = None;
    for (rule, compiled_rule) in loaded.manifest.rules.iter().zip(&loaded.compiled_rules) {
        if skip_rules.is_some_and(|ids| ids.contains(&rule.id.as_str())) {
            continue;
        }
        if (!rule.managed_launch_only || managed_launch_pending)
            && compiled_rule_matches(compiled_rule, region(input, &rule.region))
            && matched.is_none_or(|previous| previous.priority < rule.priority)
        {
            matched = Some(rule);
        }
    }
    matched
}

fn fallback_detection(managed_launch_pending: bool) -> AgentDetection {
    AgentDetection {
        // Absence of evidence can describe an idle provider after startup,
        // but cannot confirm that a managed provider has started.
        state: if managed_launch_pending {
            AgentState::Unknown
        } else {
            AgentState::Idle
        },
        skip_state_update: false,
        visible_idle: false,
        visible_blocker: false,
        visible_working: false,
    }
}

pub fn agent_state_label(state: AgentState) -> &'static str {
    match state {
        AgentState::Idle => "idle",
        AgentState::Working => "working",
        AgentState::Blocked => "blocked",
        AgentState::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::*;

    #[path = "claude_test.rs"]
    mod claude;
    #[path = "codex_test.rs"]
    mod codex;
    #[path = "engine_test.rs"]
    mod engine;
    #[path = "grok_test.rs"]
    mod grok;
    #[path = "grok_login_test.rs"]
    mod grok_login;
    #[path = "others_test.rs"]
    mod others;
    #[path = "regions_test.rs"]
    mod regions;
    #[path = "validation_test.rs"]
    mod validation;
}
