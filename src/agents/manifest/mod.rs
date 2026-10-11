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
    let Some(loaded) = load_manifest(agent) else {
        return idle_fallback();
    };
    evaluate_loaded_manifest(input, &loaded)
}

fn evaluate_loaded_manifest(input: DetectionInput<'_>, loaded: &LoadedManifest) -> AgentDetection {
    let Some(rule) = matched_manifest_rule(input, loaded) else {
        return idle_fallback();
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

/// An agent's self-update chooser that Bus answers itself for agents it
/// launched: pick the option whose label starts with `choose`, and treat the
/// agent exiting with `success` on screen as an installed update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutoUpdate {
    pub(crate) choose: String,
    pub(crate) success: String,
}

/// The self-update chooser `screen` shows, when its winning rule is one.
pub(crate) fn auto_update(agent: AgentKind, screen: &str) -> Option<AutoUpdate> {
    let loaded = load_manifest(agent)?;
    let input = DetectionInput {
        screen,
        osc_title: "",
        osc_progress: "",
    };
    let update = matched_manifest_rule(input, &loaded)?
        .auto_update
        .as_ref()?;
    Some(AutoUpdate {
        choose: update.choose.clone(),
        success: update.success.clone(),
    })
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
) -> Option<&'a ManifestRule> {
    let mut matched: Option<&ManifestRule> = None;
    for (rule, compiled_rule) in loaded.manifest.rules.iter().zip(&loaded.compiled_rules) {
        if compiled_rule_matches(compiled_rule, region(input, &rule.region))
            && matched.is_none_or(|previous| previous.priority < rule.priority)
        {
            matched = Some(rule);
        }
    }
    matched
}

fn idle_fallback() -> AgentDetection {
    AgentDetection {
        state: AgentState::Idle,
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
    #[path = "others_test.rs"]
    mod others;
    #[path = "regions_test.rs"]
    mod regions;
    #[path = "validation_test.rs"]
    mod validation;
}
