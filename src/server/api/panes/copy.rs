use super::pane_not_found;
use crate::protocol::api::schema::{
    PaneCopyMotion, PaneCopyMotionParams, PaneCopySearchDirection, PaneCopySearchParams,
    PaneScrollParams, PaneSelectionReadParams, PaneTextPoint, PaneTextRange, ResponseResult,
};
use crate::server::api::errors::{encode_error, encode_success};
use crate::server::app::App;

impl App {
    pub(in crate::server::api) fn handle_pane_scroll(
        &mut self,
        id: String,
        params: PaneScrollParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return pane_not_found(id, &params.pane_id);
        };
        runtime.set_scroll_offset_from_bottom(
            usize::try_from(params.offset_from_bottom).unwrap_or(usize::MAX),
        );
        let Some(pane) = self.pane_info(ws_idx, pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        encode_success(id, ResponseResult::PaneInfo { pane })
    }

    pub(crate) fn pane_selection_text(
        &self,
        params: &PaneSelectionReadParams,
    ) -> Result<String, (&'static str, String)> {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return Err((
                "pane_not_found",
                format!("pane not found: {}", params.pane_id),
            ));
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return Err((
                "pane_not_found",
                format!("pane not found: {}", params.pane_id),
            ));
        };
        let before = runtime.content_seq();
        if params
            .content_revision
            .is_some_and(|revision| revision != before || !before.is_multiple_of(2))
        {
            return Err(("stale_content", "pane content changed".to_owned()));
        }
        let selection = crate::utils::text::selection::Selection::absolute_range(
            pane_id,
            (params.anchor.row, params.anchor.col),
            (params.cursor.row, params.cursor.col),
        );
        let Some(text) = runtime.extract_selection(&selection) else {
            return Err((
                "selection_unavailable",
                "selection text is unavailable".to_owned(),
            ));
        };
        if params.content_revision.is_some() && runtime.content_seq() != before {
            return Err(("stale_content", "pane content changed".to_owned()));
        }
        Ok(text)
    }

    pub(in crate::server::api) fn handle_pane_selection_read(
        &mut self,
        id: String,
        params: PaneSelectionReadParams,
    ) -> String {
        match self.pane_selection_text(&params) {
            Ok(text) => encode_success(
                id,
                ResponseResult::PaneSelection {
                    pane_id: params.pane_id,
                    text,
                },
            ),
            Err((code, message)) => encode_error(id, code, message),
        }
    }

    pub(in crate::server::api) fn handle_pane_copy_motion(
        &mut self,
        id: String,
        params: PaneCopyMotionParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return pane_not_found(id, &params.pane_id);
        };
        let before = runtime.content_seq();
        if params
            .content_revision
            .is_some_and(|revision| revision != before || !before.is_multiple_of(2))
        {
            return encode_error(id, "stale_content", "pane content changed");
        }
        let target = match params.motion {
            PaneCopyMotion::LineEnd | PaneCopyMotion::FirstNonBlank => {
                let Some(target) =
                    line_copy_motion_target(runtime, pane_id, params.cursor, params.motion)
                else {
                    return encode_error(
                        id,
                        "copy_motion_unavailable",
                        "terminal row is unavailable",
                    );
                };
                target
            }
            PaneCopyMotion::NextWordStart
            | PaneCopyMotion::PreviousWordStart
            | PaneCopyMotion::NextWordEnd
            | PaneCopyMotion::NextBigWordStart
            | PaneCopyMotion::PreviousBigWordStart
            | PaneCopyMotion::NextBigWordEnd => {
                let motion = match params.motion {
                    PaneCopyMotion::NextWordStart => {
                        crate::terminal::emulator::TerminalWordMotion::NextStart
                    }
                    PaneCopyMotion::PreviousWordStart => {
                        crate::terminal::emulator::TerminalWordMotion::PreviousStart
                    }
                    PaneCopyMotion::NextWordEnd => {
                        crate::terminal::emulator::TerminalWordMotion::NextEnd
                    }
                    PaneCopyMotion::NextBigWordStart => {
                        crate::terminal::emulator::TerminalWordMotion::NextBigStart
                    }
                    PaneCopyMotion::PreviousBigWordStart => {
                        crate::terminal::emulator::TerminalWordMotion::PreviousBigStart
                    }
                    PaneCopyMotion::NextBigWordEnd => {
                        crate::terminal::emulator::TerminalWordMotion::NextBigEnd
                    }
                    _ => unreachable!(),
                };
                runtime
                    .word_motion_target(params.cursor.row, params.cursor.col, motion)
                    .unwrap_or(crate::terminal::emulator::TerminalTextPoint {
                        row: params.cursor.row,
                        col: params.cursor.col,
                    })
            }
            PaneCopyMotion::PreviousParagraph | PaneCopyMotion::NextParagraph => runtime
                .paragraph_motion_target(
                    params.cursor.row,
                    if params.motion == PaneCopyMotion::PreviousParagraph {
                        -1
                    } else {
                        1
                    },
                )
                .map(|target| crate::terminal::emulator::TerminalTextPoint {
                    row: target.row,
                    col: params.cursor.col,
                })
                .unwrap_or(crate::terminal::emulator::TerminalTextPoint {
                    row: params.cursor.row,
                    col: params.cursor.col,
                }),
        };
        let after = runtime.content_seq();
        if params.content_revision.is_some() && after != before {
            return encode_error(id, "stale_content", "pane content changed");
        }
        encode_success(
            id,
            ResponseResult::PaneCopyMotion {
                pane_id: params.pane_id,
                cursor: crate::protocol::api::schema::PaneTextPoint {
                    row: target.row,
                    col: target.col,
                },
                content_revision: after,
            },
        )
    }

    pub(in crate::server::api) fn handle_pane_copy_search(
        &mut self,
        id: String,
        params: PaneCopySearchParams,
    ) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(&params.pane_id) else {
            return pane_not_found(id, &params.pane_id);
        };
        let Some(runtime) =
            self.state
                .runtime_for_pane_in_workspace(&self.terminal_runtimes, ws_idx, pane_id)
        else {
            return pane_not_found(id, &params.pane_id);
        };
        const MAX_QUERY_BYTES: usize = 4096;
        const MAX_RETURNED_MATCHES: usize = 1024;
        if params.query.len() > MAX_QUERY_BYTES {
            return encode_error(id, "query_too_large", "copy search query is too large");
        }
        let before = runtime.content_seq();
        if before != params.content_revision || !before.is_multiple_of(2) {
            return encode_error(id, "stale_content", "pane content changed");
        }
        let cursor = crate::terminal::emulator::TerminalTextPoint {
            row: params.cursor.row,
            col: params.cursor.col,
        };
        let previous = params.previous.map(|previous| {
            (
                crate::terminal::emulator::TerminalTextPoint {
                    row: previous.start.row,
                    col: previous.start.col,
                },
                crate::terminal::emulator::TerminalTextPoint {
                    row: previous.end.row,
                    col: previous.end.col,
                },
            )
        });
        let direction = match params.direction {
            PaneCopySearchDirection::Forward => {
                crate::terminal::emulator::TerminalSearchDirection::Forward
            }
            PaneCopySearchDirection::Backward => {
                crate::terminal::emulator::TerminalSearchDirection::Backward
            }
        };
        let result = runtime.search_text_window(
            &params.query,
            params.query.chars().any(char::is_uppercase),
            direction,
            cursor,
            previous,
            MAX_RETURNED_MATCHES,
        );
        let after = runtime.content_seq();
        if after != before || !after.is_multiple_of(2) {
            return encode_error(id, "stale_content", "pane content changed");
        }
        let matches = result
            .matches
            .into_iter()
            .map(|text_match| PaneTextRange {
                start: PaneTextPoint {
                    row: text_match.start.row,
                    col: text_match.start.col,
                },
                end: PaneTextPoint {
                    row: text_match.end.row,
                    col: text_match.end.col,
                },
            })
            .collect();
        encode_success(
            id,
            ResponseResult::PaneCopySearch {
                pane_id: params.pane_id,
                content_revision: after,
                matches,
                total: u64::try_from(result.total).unwrap_or(u64::MAX),
                current: result.current.and_then(|index| u32::try_from(index).ok()),
                current_global: result
                    .current_global
                    .and_then(|index| u64::try_from(index).ok()),
            },
        )
    }
}

fn line_copy_motion_target(
    runtime: &crate::terminal::TerminalRuntime,
    pane_id: crate::utils::ids::PaneId,
    cursor: PaneTextPoint,
    motion: PaneCopyMotion,
) -> Option<crate::terminal::emulator::TerminalTextPoint> {
    let width = runtime
        .terminal_dimensions()
        .map_or(1, |(cols, _)| cols.max(1));
    let selection = crate::utils::text::selection::Selection::absolute_range(
        pane_id,
        (cursor.row, 0),
        (cursor.row, width.saturating_sub(1)),
    );
    let text = runtime.extract_selection(&selection)?;
    let col = match motion {
        PaneCopyMotion::LineEnd => {
            crate::utils::text::copy_motion::last_character_col(&text, |ch| {
                u16::from(crate::utils::text::width::unicode_codepoint_width(
                    ch as u32,
                ))
            })
            .map_or(Some(0), |_| {
                last_character_cell(runtime, pane_id, cursor.row, width, &text)
            })?
        }
        PaneCopyMotion::FirstNonBlank => {
            crate::utils::text::copy_motion::first_non_blank_col(&text, |ch| {
                u16::from(crate::utils::text::width::unicode_codepoint_width(
                    ch as u32,
                ))
            })
            .unwrap_or(0)
        }
        _ => unreachable!(),
    };
    Some(crate::terminal::emulator::TerminalTextPoint {
        row: cursor.row,
        col: col.min(width.saturating_sub(1)),
    })
}

/// Grapheme clusters can span fewer cells than their codepoints' widths add up to, so ask the
/// terminal: the last character starts at the first column whose prefix already reads the whole row.
fn last_character_cell(
    runtime: &crate::terminal::TerminalRuntime,
    pane_id: crate::utils::ids::PaneId,
    row: u32,
    width: u16,
    text: &str,
) -> Option<u16> {
    let (mut low, mut high) = (0u16, width.saturating_sub(1));
    while low < high {
        let mid = low + (high - low) / 2;
        let prefix =
            crate::utils::text::selection::Selection::absolute_range(pane_id, (row, 0), (row, mid));
        if runtime.extract_selection(&prefix)? == text {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    Some(low)
}
