//! Find selection follows the logical message when older pages prepend rows.
use super::{History, Line, MarkdownSource, RowAnchor, RowKind};
use crate::messaging::model::{BusState, PromptId};
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

fn identity(line: &Line, state: &BusState) -> RowAnchor {
    let mut anchor = line.anchor;
    if matches!(
        anchor.kind,
        RowKind::ReplyHeader | RowKind::ReplyBody | RowKind::Quote
    ) {
        let request = match &line.raw_markdown {
            Some((MarkdownSource::Reply(id), _)) => state.request(*id),
            _ => state
                .requests_for_message(anchor.prompt)
                .find(|request| Some(request.agent_id) == anchor.agent),
        };
        if let Some(request) = request {
            // A shared body may move to an older group member as the page grows.
            anchor.prompt = PromptId(request.reply_key().0);
        }
    }
    anchor
}
