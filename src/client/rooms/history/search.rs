//! Find selection follows the logical message when older pages prepend rows.
use super::{History, Line, RowAnchor};
use crate::messaging::model::BusState;
use std::ops::Range;

pub(in crate::client::rooms) struct SearchAnchor {
    row: RowAnchor,
    range: Range<usize>,
    visible: bool,
}

impl SearchAnchor {
    pub(in crate::client::rooms) fn capture(
        history: &History,
        state: &BusState,
        current: Option<usize>,
        matches: &[(usize, Range<usize>)],
        height: usize,
        scroll: usize,
    ) -> Option<Self> {
        let (row, range) = matches.get(current?)?;
        Some(Self {
            row: identity(history.cached().get(*row)?, state),
            range: range.clone(),
            visible: (scroll..scroll + height).contains(row),
        })
    }

    pub(in crate::client::rooms) fn restore(
        self,
        history: &History,
        state: &BusState,
        matches: &[(usize, Range<usize>)],
        height: usize,
        scroll: &mut usize,
    ) -> Option<usize> {
        let index = matches.iter().position(|(row, range)| {
            *range == self.range
                && history
                    .cached()
                    .get(*row)
                    .is_some_and(|line| identity(line, state) == self.row)
        })?;
        if self.visible {
            let row = matches[index].0;
            if !(*scroll..scroll.saturating_add(height)).contains(&row) {
                *scroll = row.saturating_sub(height / 3);
            }
        }
        Some(index)
    }
}

fn identity(line: &Line, _state: &BusState) -> RowAnchor {
    // History rows are the message itself. A reply is not a separate row whose
    // anchor has to follow a group member onto an older page.
    line.anchor
}
