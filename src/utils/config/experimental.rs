use serde::Deserialize;

/// Cursor shape (DECSCUSR) used for the forced IME anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImeCursorShape {
    Block,
    #[default]
    SteadyBlock,
    Underline,
    SteadyUnderline,
    Bar,
    SteadyBar,
}

impl ImeCursorShape {
    /// Convert to DECSCUSR parameter (1–6).
    pub fn to_decscusr(self) -> u8 {
        match self {
            Self::Block => 1,
            Self::SteadyBlock => 2,
            Self::Underline => 3,
            Self::SteadyUnderline => 4,
            Self::Bar => 5,
            Self::SteadyBar => 6,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExperimentalConfig {
    /// Persist pane screen history to session-history.json. Default: false.
    pub pane_history: bool,
    /// Expose the focused pane's cursor anchor to the outer terminal even when
    /// the pane requested `?25l`, so macOS native input methods keep tracking
    /// the candidate window when TUIs paint their own cursor (Claude Code, pi,
    /// codex, etc.). Default: false.
    ///
    /// When the pane reports no cursor position, falls back to the pane's
    /// top-left so a stable IME anchor is always available.
    ///
    /// Trade-off when enabled: an extra hardware cursor will be visible in the
    /// outer terminal for apps that hide the cursor without painting a
    /// replacement (vim normal mode, etc.). See #149.
    pub reveal_hidden_cursor_for_cjk_ime: bool,
    /// Restrict `reveal_hidden_cursor_for_cjk_ime` to focused panes whose
    /// detected agent matches one of these names (case-insensitive). Empty
    /// list means apply to any focused pane. Unknown agent names are ignored;
    /// if the list contains no valid names, the reveal does not apply.
    /// Accepted names: pi, claude, codex, gemini, cursor, devin, cline,
    /// opencode, copilot, kimi, kiro, droid, amp, grok, hermes, kilo,
    /// qodercli, qoder, qwen, qwen-code, maki.
    /// Default: empty.
    pub cjk_ime_agents: Vec<String>,
    /// Cursor shape rendered for the IME anchor when
    /// `reveal_hidden_cursor_for_cjk_ime` is enabled. Default: "steady_block".
    pub cjk_ime_cursor_shape: ImeCursorShape,
    /// While prefix mode is active, temporarily switch the host input source
    /// to an ASCII-capable mode so prefix commands are read as ASCII even when
    /// an IME is active, then restore the previous input source when prefix
    /// mode exits. On macOS this selects the ASCII-capable keyboard layout; on
    /// Windows it switches the IME to English (ASCII) input. Windows support is
    /// currently limited to the Korean IME; with an IME for any other language,
    /// the input source is left unchanged. macOS and Windows only; a no-op
    /// elsewhere and a best-effort no-op if the switch fails.
    /// Default: false.
    pub switch_ascii_input_source_in_prefix: bool,
}
