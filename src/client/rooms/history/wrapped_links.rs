//! Keep complete link targets when Markdown divides their text over rows.
use super::{Line, LinkScope};
use crate::client::rooms::links::{candidates, Candidate};
use ratatui::style::Modifier;
use std::ops::Range;

type Fragments = Vec<(usize, Range<usize>)>;

/// Source correspondence is built with the cached history, never by scanning
/// off-screen messages on every frame. The existing detector and path probe
/// still decide which tokens can link and whether their targets exist.
pub(super) fn scoped_lines(lines: &[Line], scope: LinkScope) -> Vec<Line> {
    let _timer = super::phases::LINKS.start();
    let mut rows = lines.to_vec();
    let Some((_, source)) = lines.first().and_then(|line| line.raw_markdown.as_ref()) else {
        return rows;
    };
    let home = std::env::home_dir();
    let tokens = candidates(source, scope.cwd.as_deref(), home.as_deref());
    let ranges: Vec<_> = tokens.iter().map(|token| token.range.clone()).collect();
    let fragments = source_fragments(lines, source, &ranges);
    for line in &mut rows {
        line.links = Some(scope.clone());
    }
    for (token, fragments) in tokens.into_iter().zip(fragments) {
        let Some((first, _)) = fragments.first() else {
            continue;
        };
        if fragments.last().is_none_or(|(last, _)| last == first) {
            continue;
        }
        let mut previous_row = None;
        for (row, range) in fragments {
            let Some(scope) = &mut rows[row].links else {
                continue;
            };
            if let Some(last) = scope
                .wrapped
                .last_mut()
                .filter(|_| previous_row == Some(row))
            {
                // Include spaces inside one quoted path, but never text past
                // the token's source range.
                last.range.end = range.end;
            } else {
                scope.wrapped.push(Candidate {
                    range,
                    target: token.target.clone(),
                });
            }
            previous_row = Some(row);
        }
    }
    rows
}

/// Map complete source tokens onto styled rows. Hidden or partly represented
/// tokens must not attach links or foregrounds to unrelated visible words.
pub(super) fn source_fragments(
    lines: &[Line],
    source: &str,
    ranges: &[Range<usize>],
) -> Vec<Fragments> {
    let mut fragments = vec![Vec::new(); ranges.len()];
    let mut covered = vec![0; ranges.len()];
    let mut cursor = 0;
    for (row, line) in lines.iter().enumerate() {
        let mut offset = 0;
        let runs: Vec<_> = if line.styles.is_empty() {
            vec![(line.text.as_str(), false)]
        } else {
            line.styles
                .iter()
                .map(|(text, style)| (text.as_str(), style.add_modifier.contains(Modifier::DIM)))
                .collect()
        };
        for (text, layout) in runs {
            // DIM runs are Markdown's generated bullets, quote/code borders
            // and fence labels. They have no source-text correspondence.
            if !layout {
                for (start, word) in words(text) {
                    let start = offset + start;
                    if start < line.copy_from {
                        continue;
                    }
                    let Some(found) = source[cursor..].find(word) else {
                        continue;
                    };
                    let from = cursor + found;
                    let to = from + word.len();
                    for (index, range) in ranges.iter().enumerate() {
                        let left = from.max(range.start);
                        let right = to.min(range.end);
                        if left < right {
                            fragments[index].push((row, start + left - from..start + right - from));
                            covered[index] += source[left..right]
                                .chars()
                                .filter(|c| !c.is_whitespace())
                                .count();
                        }
                    }
                    cursor = to;
                }
            }
            offset += text.len();
        }
    }
    for ((fragments, range), covered) in fragments.iter_mut().zip(ranges).zip(covered) {
        if covered
            != source[range.clone()]
                .chars()
                .filter(|c| !c.is_whitespace())
                .count()
        {
            fragments.clear();
        }
    }
    fragments
}

fn words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split_inclusive(char::is_whitespace)
        .filter_map(move |part| {
            let start = offset;
            offset += part.len();
            let word = part.trim_end();
            (!word.is_empty()).then_some((start, word))
        })
}
