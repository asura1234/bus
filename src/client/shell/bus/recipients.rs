//! Whole-chip wrapping in terminal cells. No fixed limit on the number of rows.
use crate::bus::model::AgentId;
use ratatui::layout::Rect;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) struct Chip {
    pub agent: AgentId,
    pub label: String,
    pub rect: Rect,
}

pub(super) struct Layout {
    pub chips: Vec<Chip>,
    pub height: u16,
}

pub(super) fn layout<'a>(names: impl Iterator<Item = (AgentId, &'a str)>, width: u16) -> Layout {
    let mut chips = Vec::new();
    let mut x = 6;
    let mut y: u16 = 0;
    for (agent, name) in names {
        if width < 10 {
            break;
        }
        let name = super::render::display(name);
        let chip_width = (name.width().saturating_add(4)).min(usize::from(width - 6)) as u16;
        if x + chip_width > width {
            x = 6;
            y = y.saturating_add(3);
        }
        let label = clip(&name, chip_width.saturating_sub(4));
        chips.push(Chip {
            agent,
            label,
            rect: Rect::new(x, y, chip_width, 3),
        });
        x = x.saturating_add(chip_width + 1);
    }
    let height = chips.last().map_or(1, |chip| chip.rect.bottom());
    Layout { chips, height }
}

fn clip(name: &str, width: u16) -> String {
    if name.width() <= usize::from(width) {
        return name.to_owned();
    }
    let mut used = 0;
    let mut text: String = name
        .chars()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= usize::from(width.saturating_sub(1))
        })
        .collect();
    text.push('…');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wide_names_stay_inside_one_chip_without_splitting_rows() {
        let items = [
            (AgentId(1), "非常非常非常非常长的名称"),
            (AgentId(2), "short"),
        ];
        let layout = layout(items.into_iter(), 18);
        assert_eq!(layout.chips.len(), 2);
        for chip in &layout.chips {
            assert!(chip.rect.right() <= 18);
            assert!(chip.label.width() <= usize::from(chip.rect.width - 4));
            assert_eq!(chip.rect.height, 3);
        }
        assert!(layout.chips[0].label.ends_with('…'));
        assert!(layout.chips[1].rect.y >= layout.chips[0].rect.bottom());
    }
}
