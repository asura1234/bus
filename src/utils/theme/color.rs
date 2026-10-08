#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostAppearance {
    Dark,
    Light,
}

impl HostAppearance {
    pub const fn color_scheme_report(self) -> &'static [u8] {
        match self {
            Self::Dark => b"\x1b[?997;1n",
            Self::Light => b"\x1b[?997;2n",
        }
    }
}

impl RgbColor {
    pub fn inferred_appearance(self) -> HostAppearance {
        let luminance = u32::from(self.r) * 299 + u32::from(self.g) * 587 + u32::from(self.b) * 114;
        if luminance >= 128_000 {
            HostAppearance::Light
        } else {
            HostAppearance::Dark
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalTheme {
    pub foreground: Option<RgbColor>,
    pub background: Option<RgbColor>,
    pub palette: [Option<RgbColor>; 256],
}

impl Default for TerminalTheme {
    fn default() -> Self {
        Self {
            foreground: None,
            background: None,
            palette: [None; 256],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultColorKind {
    Foreground,
    Background,
}

#[cfg(any(not(windows), test))]
pub use crate::client::host_terminal::color_probe::HOST_COLOR_SCHEME_QUERY_SEQUENCE;
pub use crate::client::host_terminal::color_probe::{
    host_terminal_theme_query_sequence, osc_reset_default_color_sequence,
    osc_set_default_color_sequence, parse_default_color_response, parse_palette_color_response,
    HOST_COLOR_SCHEME_REPORT_DISABLE_SEQUENCE, HOST_COLOR_SCHEME_REPORT_ENABLE_SEQUENCE,
};
#[cfg(test)]
use crate::client::host_terminal::color_probe::{parse_hex_component, HOST_COLOR_QUERY_SEQUENCE};

impl TerminalTheme {
    pub fn with_color(mut self, kind: DefaultColorKind, color: RgbColor) -> Self {
        match kind {
            DefaultColorKind::Foreground => self.foreground = Some(color),
            DefaultColorKind::Background => self.background = Some(color),
        }
        self
    }

    pub fn with_palette_color(mut self, index: u8, color: RgbColor) -> Self {
        self.palette[usize::from(index)] = Some(color);
        self
    }

    pub fn is_empty(self) -> bool {
        self.foreground.is_none() && self.background.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_st_terminated_rgb_response() {
        let parsed = parse_default_color_response("\x1b]10;rgb:cccc/dddd/eeee\x1b\\");
        assert_eq!(
            parsed,
            Some((
                DefaultColorKind::Foreground,
                RgbColor {
                    r: 0xcc,
                    g: 0xdd,
                    b: 0xee,
                },
            ))
        );
    }

    #[test]
    fn parses_bel_terminated_hash_response() {
        let parsed = parse_default_color_response("\x1b]11;#123456\u{7}");
        assert_eq!(
            parsed,
            Some((
                DefaultColorKind::Background,
                RgbColor {
                    r: 0x12,
                    g: 0x34,
                    b: 0x56,
                },
            ))
        );
    }

    #[test]
    fn parses_palette_responses_and_builds_full_query() {
        assert_eq!(
            parse_palette_color_response("\x1b]4;255;rgb:1111/2222/3333\x1b\\"),
            Some((
                255,
                RgbColor {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                }
            ))
        );

        let query = host_terminal_theme_query_sequence(true);
        assert!(query.starts_with(HOST_COLOR_QUERY_SEQUENCE));
        assert!(query.contains("\x1b]4;0;?\x1b\\"));
        assert!(query.ends_with("\x1b]4;255;?\x1b\\"));
        assert_eq!(query.matches("\x1b]4;").count(), 256);

        assert_eq!(
            host_terminal_theme_query_sequence(false),
            HOST_COLOR_QUERY_SEQUENCE
        );
    }

    #[test]
    fn default_color_reset_sequences_use_xterm_osc_numbers() {
        assert_eq!(
            osc_reset_default_color_sequence(DefaultColorKind::Foreground),
            "\x1b]110\x1b\\"
        );
        assert_eq!(
            osc_reset_default_color_sequence(DefaultColorKind::Background),
            "\x1b]111\x1b\\"
        );
    }

    #[test]
    fn scales_short_hex_components() {
        assert_eq!(parse_hex_component("f"), Some(255));
        assert_eq!(parse_hex_component("80"), Some(128));
        assert_eq!(parse_hex_component("800"), Some(128));
        assert_eq!(parse_hex_component("8000"), Some(128));
    }
}
