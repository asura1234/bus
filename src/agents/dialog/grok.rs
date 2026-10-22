//! Lane C owns Grok cards, focus and composer exclusions.
use super::{Dialog, Style};
pub(super) fn parse(_lines: &[&str], _styles: &[Vec<Style>]) -> Option<Dialog> {
    None
}
pub(super) fn contains_composer(_lines: &[&str]) -> bool {
    false
}
