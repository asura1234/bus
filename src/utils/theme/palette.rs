use ratatui::style::Color;

// ---------------------------------------------------------------------------
// Theme palette — all UI colors in one place, ready for theming
// ---------------------------------------------------------------------------

/// All colors used by the UI. Derived from a base accent color for now,
/// but structured so a full theme system can replace it later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// Primary accent (highlight, active borders).
    pub accent: Color,
    /// Background for the tab bar, floating panels, overlays, and modals.
    pub panel_bg: Color,
    /// Optional desktop sidebar background. Reset preserves the terminal background.
    pub sidebar_bg: Color,
    /// Background for the active workspace and focused agent rows.
    pub active_row_bg: Color,
    /// Background for the Navigate-mode cursor row in the sidebar.
    pub selection_bg: Color,
    /// Subtle surface background for selected/focused items.
    pub surface0: Color,
    /// Slightly lighter surface for hover/active states.
    pub surface1: Color,
    /// Very dim surface for separators.
    pub surface_dim: Color,
    /// Muted text (secondary info, numbers).
    pub overlay0: Color,
    /// Slightly brighter overlay text.
    pub overlay1: Color,
    /// Main text color — soft white.
    pub text: Color,
    /// Subdued text (workspace numbers, dim labels).
    pub subtext0: Color,
    /// Branch name / special label color.
    pub mauve: Color,
    /// Done / idle states.
    pub green: Color,
    /// Working / running states.
    pub yellow: Color,
    /// Needs attention / blocked states.
    pub red: Color,
    /// Unseen / done notification accent.
    pub blue: Color,
    /// Notification accent / unseen markers.
    pub teal: Color,
    /// Interrupted / warning states.
    pub peach: Color,
}

impl Palette {
    /// Resolve a theme by name. Returns None for unknown names.
    pub fn from_name(name: &str) -> Option<Self> {
        match crate::utils::config::canonical_theme_name(name)? {
            "catppuccin" => Some(Self::catppuccin()),
            "catppuccin-latte" => Some(Self::catppuccin_latte()),
            "terminal" => Some(Self::terminal()),
            "tokyo-night" => Some(Self::tokyo_night()),
            "tokyo-night-day" => Some(Self::tokyo_night_day()),
            "dracula" => Some(Self::dracula()),
            "nord" => Some(Self::nord()),
            "gruvbox" => Some(Self::gruvbox()),
            "gruvbox-light" => Some(Self::gruvbox_light()),
            "one-dark" => Some(Self::one_dark()),
            "one-light" => Some(Self::one_light()),
            "solarized" => Some(Self::solarized()),
            "solarized-light" => Some(Self::solarized_light()),
            "kanagawa" => Some(Self::kanagawa()),
            "kanagawa-lotus" => Some(Self::kanagawa_lotus()),
            "rose-pine" => Some(Self::rose_pine()),
            "rose-pine-dawn" => Some(Self::rose_pine_dawn()),
            "vesper" => Some(Self::vesper()),
            _ => None,
        }
    }

    /// Apply custom color overrides on top of this palette.
    pub fn with_overrides(mut self, custom: &crate::utils::config::CustomThemeColors) -> Self {
        apply_color_overrides([
            (&mut self.accent, custom.accent.as_deref()),
            (&mut self.panel_bg, custom.panel_bg.as_deref()),
            (&mut self.sidebar_bg, custom.sidebar_bg.as_deref()),
            (&mut self.active_row_bg, custom.active_row_bg.as_deref()),
            (&mut self.selection_bg, custom.selection_bg.as_deref()),
            (&mut self.surface0, custom.surface0.as_deref()),
            (&mut self.surface1, custom.surface1.as_deref()),
            (&mut self.surface_dim, custom.surface_dim.as_deref()),
            (&mut self.overlay0, custom.overlay0.as_deref()),
            (&mut self.overlay1, custom.overlay1.as_deref()),
            (&mut self.text, custom.text.as_deref()),
            (&mut self.subtext0, custom.subtext0.as_deref()),
            (&mut self.mauve, custom.mauve.as_deref()),
            (&mut self.green, custom.green.as_deref()),
            (&mut self.yellow, custom.yellow.as_deref()),
            (&mut self.red, custom.red.as_deref()),
            (&mut self.blue, custom.blue.as_deref()),
            (&mut self.teal, custom.teal.as_deref()),
            (&mut self.peach, custom.peach.as_deref()),
        ]);
        self
    }

    pub fn with_mode_overrides(mut self, custom: &crate::utils::config::ModeThemeColors) -> Self {
        apply_color_overrides([
            (&mut self.accent, custom.accent.as_deref()),
            (&mut self.panel_bg, custom.panel_bg.as_deref()),
            (&mut self.sidebar_bg, custom.sidebar_bg.as_deref()),
            (&mut self.active_row_bg, custom.active_row_bg.as_deref()),
            (&mut self.selection_bg, custom.selection_bg.as_deref()),
            (&mut self.surface0, custom.surface0.as_deref()),
            (&mut self.surface1, custom.surface1.as_deref()),
            (&mut self.surface_dim, custom.surface_dim.as_deref()),
            (&mut self.overlay0, custom.overlay0.as_deref()),
            (&mut self.overlay1, custom.overlay1.as_deref()),
            (&mut self.text, custom.text.as_deref()),
            (&mut self.subtext0, custom.subtext0.as_deref()),
            (&mut self.mauve, custom.mauve.as_deref()),
            (&mut self.green, custom.green.as_deref()),
            (&mut self.yellow, custom.yellow.as_deref()),
            (&mut self.red, custom.red.as_deref()),
            (&mut self.blue, custom.blue.as_deref()),
            (&mut self.teal, custom.teal.as_deref()),
            (&mut self.peach, custom.peach.as_deref()),
        ]);
        self
    }
}

fn apply_color_overrides<const N: usize>(fields: [(&mut Color, Option<&str>); N]) {
    for (target, value) in fields {
        if let Some(value) = value {
            *target = crate::utils::config::parse_color(value);
        }
    }
}
