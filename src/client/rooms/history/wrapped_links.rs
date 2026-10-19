//! Keep complete link targets when Markdown divides their text over rows.
use super::{Line, LinkScope};
use crate::client::rooms::links::{candidates, Candidate};
use ratatui::style::Modifier;

/// Source correspondence is built with the cached history, never by scanning
/// off-screen messages on every frame. The existing detector and path probe
/// still decide which tokens can link and whether their targets exist.
pub(super) fn scoped_lines(lines: &[Line], scope: LinkScope) -> Vec<Line> {
    let mut rows = lines.to_vec();
    let Some((_, source)) = lines.first().and_then(|line| line.raw_markdown.as_ref()) else {
        return rows;
    };
    let home = std::env::home_dir();
    let tokens = candidates(source, scope.cwd.as_deref(), home.as_deref());
    let mut fragments = vec![Vec::new(); tokens.len()];
    let mut covered = vec![0; tokens.len()];
    let mut cursor = 0;
    for (row, line) in rows.iter().enumerate() {
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
                    for (index, token) in tokens.iter().enumerate() {
                        let left = from.max(token.range.start);
                        let right = to.min(token.range.end);
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
    for line in &mut rows {
        line.links = Some(scope.clone());
    }
    for ((token, fragments), covered) in tokens.into_iter().zip(fragments).zip(covered) {
        let Some((first, _)) = fragments.first() else {
            continue;
        };
        if fragments.last().is_none_or(|(last, _)| last == first) {
            continue;
        }
        if covered
            != source[token.range.clone()]
                .chars()
                .filter(|c| !c.is_whitespace())
                .count()
        {
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

pub(super) fn words(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split_inclusive(char::is_whitespace)
        .filter_map(move |part| {
            let start = offset;
            offset += part.len();
            let word = part.trim_end();
            (!word.is_empty()).then_some((start, word))
        })
}
