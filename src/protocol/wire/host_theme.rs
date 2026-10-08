use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHostColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<crate::terminal_theme::RgbColor> for ClientHostColor {
    fn from(color: crate::terminal_theme::RgbColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
        }
    }
}

impl From<ClientHostColor> for crate::terminal_theme::RgbColor {
    fn from(color: ClientHostColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostDefaultColorKind {
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostAppearance {
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientHostThemeUpdate {
    DefaultColor {
        kind: ClientHostDefaultColorKind,
        color: ClientHostColor,
    },
    PaletteColors(Vec<(u8, ClientHostColor)>),
    Appearance(ClientHostAppearance),
}
