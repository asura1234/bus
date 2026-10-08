use crossterm::event::KeyModifiers;
use serde::{de, Deserialize, Deserializer, Serialize};
use std::num::NonZeroUsize;

use super::toast::ToastConfig;
use super::{SoundConfig, DEFAULT_MOBILE_WIDTH_THRESHOLD, DEFAULT_MOUSE_SCROLL_LINES};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentPanelSortConfig {
    #[default]
    Spaces,
    Priority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostCursorModeConfig {
    #[default]
    Auto,
    Native,
    Drawn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RightClickPassthroughModifierConfig(Option<KeyModifiers>);

impl RightClickPassthroughModifierConfig {
    pub fn modifiers(self) -> Option<KeyModifiers> {
        self.0
    }
}

impl<'de> Deserialize<'de> for RightClickPassthroughModifierConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_right_click_passthrough_modifier(&value)
            .map(Self)
            .ok_or_else(|| {
                de::Error::custom(
                    "right_click_passthrough_modifier must be empty, off, none, disabled, ctrl/control, alt/option, cmd/command/super, meta, hyper, or a + separated combination without shift",
                )
            })
    }
}

fn parse_right_click_passthrough_modifier(value: &str) -> Option<Option<KeyModifiers>> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("off")
        || trimmed.eq_ignore_ascii_case("none")
        || trimmed.eq_ignore_ascii_case("disabled")
    {
        return Some(None);
    }

    let mut modifiers = KeyModifiers::empty();
    for token in trimmed.split('+') {
        let token = token.trim().to_ascii_lowercase();
        let modifier = match token.as_str() {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" | "option" => KeyModifiers::ALT,
            "cmd" | "command" | "super" => KeyModifiers::SUPER,
            "meta" => KeyModifiers::META,
            "hyper" => KeyModifiers::HYPER,
            _ => return None,
        };
        modifiers |= modifier;
    }

    Some(Some(modifiers))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TabBarPositionConfig {
    #[default]
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneBordersConfig {
    #[default]
    Auto,
    Always,
    Off,
}

impl PaneBordersConfig {
    pub fn draws_borders(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn shows_borders(self, multi_pane: bool) -> bool {
        self.draws_borders() && (multi_pane || matches!(self, Self::Always))
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// Terminal width at or below which Herdr uses the mobile single-column layout. Default: 64.
    pub mobile_width_threshold: u16,
    /// Capture mouse input for Herdr's mouse UI. Default: true.
    pub mouse_capture: bool,
    /// Copy text selected with the mouse. Default: true.
    pub copy_on_select: bool,
    /// Host cursor policy. Default: auto.
    pub host_cursor: HostCursorModeConfig,
    /// Modifier that lets right-click gestures pass through to pane apps. Empty disables it.
    pub right_click_passthrough_modifier: RightClickPassthroughModifierConfig,
    /// Force a full host-terminal redraw when the outer terminal regains focus. Default: true.
    pub redraw_on_focus_gained: bool,
    /// Lines to scroll per mouse wheel notch. Default: 3.
    pub mouse_scroll_lines: Option<NonZeroUsize>,
    /// Ask for confirmation before closing a workspace. Default: true.
    pub confirm_close: bool,
    /// Ask for a tab name before creating a new tab. Default: true.
    pub prompt_new_tab_name: bool,
    /// Ask for a workspace name before interactive creation. Default: false.
    pub prompt_new_workspace_name: bool,
    /// Draw borders around split panes. auto draws them only for split panes,
    /// always also frames a lone pane (only while pane_outer_borders is
    /// enabled, since every edge of a lone pane is an outer edge), off
    /// disables them. Default: auto.
    pub pane_borders: PaneBordersConfig,
    /// Draw borders along the outside edge of the pane area. Default: true.
    pub pane_outer_borders: bool,
    /// Draw interactive scrollbars beside terminal panes. Default: true.
    pub pane_scrollbars: bool,
    /// Keep split panes visually separated instead of sharing divider borders. Default: true.
    pub pane_gaps: bool,
    /// Show agent labels in split pane borders when no manual pane label is set. Default: false.
    pub show_agent_labels_on_pane_borders: bool,
    /// Hide the tab row when the workspace has one tab. Default: false.
    pub hide_tab_bar_when_single_tab: bool,
    /// Desktop tab row placement. Default: top.
    pub tab_bar_position: TabBarPositionConfig,
    /// Format for the outer terminal window title. Empty leaves the title alone.
    /// Default: "{hostname}: {workspace}".
    pub window_title: String,
    /// Agent sidebar ordering. Saved values are "spaces" or "priority". Default: "spaces".
    pub agent_panel_sort: AgentPanelSortConfig,
    /// Accent color for highlights, borders, and navigation UI.
    /// Accepts hex (#89b4fa), named colors (cyan, blue), or RGB (rgb(137,180,250)).
    pub accent: String,
    /// Optional visual toast notifications for background workspace events.
    pub toast: ToastConfig,
    /// Play sounds when agents change state in background workspaces.
    pub sound: SoundConfig,
}

/// Cursor shape (DECSCUSR) used for the forced IME anchor.
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            mobile_width_threshold: DEFAULT_MOBILE_WIDTH_THRESHOLD,
            mouse_capture: true,
            copy_on_select: true,
            host_cursor: HostCursorModeConfig::Auto,
            right_click_passthrough_modifier: RightClickPassthroughModifierConfig::default(),
            redraw_on_focus_gained: true,
            mouse_scroll_lines: None,
            confirm_close: true,
            prompt_new_tab_name: true,
            prompt_new_workspace_name: false,
            pane_borders: PaneBordersConfig::Auto,
            pane_outer_borders: true,
            pane_scrollbars: true,
            pane_gaps: true,
            show_agent_labels_on_pane_borders: false,
            hide_tab_bar_when_single_tab: false,
            tab_bar_position: TabBarPositionConfig::Top,
            window_title: super::ui::window_title::default_window_title(),
            agent_panel_sort: AgentPanelSortConfig::Spaces,
            accent: "cyan".into(),
            toast: ToastConfig::default(),
            sound: SoundConfig::default(),
        }
    }
}

impl UiConfig {
    pub fn mouse_scroll_lines(&self) -> usize {
        self.mouse_scroll_lines
            .map(NonZeroUsize::get)
            .unwrap_or(DEFAULT_MOUSE_SCROLL_LINES)
    }

    pub fn right_click_passthrough_modifiers(&self) -> Option<KeyModifiers> {
        self.right_click_passthrough_modifier.modifiers()
    }
}
