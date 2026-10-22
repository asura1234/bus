use super::{Notice, Page, PullRequest, Scope, Watermark, LOOKBACK_MS};
use std::collections::BTreeSet;

/// Overrides also accept bare coordinates; remote discovery must require a GitHub host.
pub(crate) fn repository(value: &str) -> Option<String> {
    let path = remote_path(value)
        .unwrap_or(value.trim())
        .trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    if [owner, name].iter().any(|s| {
        s.is_empty()
            || matches!(*s, "." | "..")
            || !s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }) {
        return None;
    }
    Some(format!("{owner}/{name}").to_ascii_lowercase())
}

pub(super) fn remote_repository(value: &str) -> Option<String> {
    repository(remote_path(value)?)
}
fn remote_path(value: &str) -> Option<&str> {
    let value = value.trim();
    [
        "https://github.com/",
        "http://github.com/",
        "git://github.com/",
        "git@github.com:",
        "github.com:",
        "ssh://git@github.com/",
        "ssh://github.com/",
    ]
    .iter()
    .find_map(|prefix| value.strip_prefix(prefix))
}

fn owns(scope: &Scope, scopes: &[Scope], page: &Page, pr: &PullRequest) -> bool {
    let head_match =
        |s: &Scope| pr.branch != page.default_branch && s.branches.contains(&pr.branch);
    if scopes.iter().any(head_match) {
        return head_match(scope);
    }
    let authors: BTreeSet<_> = scopes
        .iter()
        .filter(|s| {
            s.branches
                .iter()
                .filter(|b| *b != &page.default_branch)
                .any(|branch| {
                    page.branch_authors
                        .get(branch)
                        .is_some_and(|author| author.eq_ignore_ascii_case(&pr.author))
                })
        })
        .map(|s| s.room)
        .collect();
    authors.len() == 1 && authors.contains(&scope.room)
}
fn notice(room: u64, repo: &str, text: String, actionable: bool) -> Notice {
    Notice {
        room,
        actionable,
        text: format!("GitHub: {repo} {text}"),
    }
}
fn merge_time(ms: u64, now: u64) -> String {
    let local = |at: u64| {
        i64::try_from(at / 1000)
            .ok()
            .and_then(crate::platform::local_datetime_at)
    };
    local(ms)
        .map(|time| {
            let clock = format!("{:02}:{:02}", time.hour(), time.minute());
            if local(now).is_some_and(|today| today.date() == time.date()) {
                clock
            } else {
                format!("{} {clock}", time.date())
            }
        })
        .unwrap_or_else(|| "unknown time".into())
}

pub(crate) fn changes(
    repo: &str,
    mark: &mut Watermark,
    page: &Page,
    scopes: &[Scope],
    now: u64,
) -> Vec<Notice> {
    let mut notices = Vec::new();
    if mark.scan_started_ms == 0 {
        mark.scan_started_ms = now;
    }
    // Eligibility is fixed at watch creation. Pagination is ordered by updatedAt,
    // so a merge skipped when it moves pages remains eligible next scan.
    let since = *mark
        .merged_after_ms
        .get_or_insert_with(|| mark.scan_started_ms.saturating_sub(LOOKBACK_MS));
    let fresh: Vec<_> = page
        .merged
        .iter()
        .filter(|pr| {
            pr.base == page.default_branch
                && pr.merged_at_ms.is_some_and(|t| t >= since)
                && !mark.merged_seen.contains_key(&pr.number)
        })
        .collect();
    for scope in scopes {
        notices.extend(merge_changes(repo, scope, scopes, page, &fresh, now));
        let observed = mark.observed.entry(scope.room).or_default();
        notices.extend(open_changes(repo, scope, scopes, page, observed));
    }
    for pr in fresh {
        mark.merged_seen
            .insert(pr.number, pr.merged_at_ms.unwrap_or(now));
    }
    mark.merged_cursor.clone_from(&page.merged_cursor);
    if mark.merged_cursor.is_none() {
        mark.merged_since_ms = mark.scan_started_ms;
        mark.scan_started_ms = 0;
    }
    mark.open_cursor.clone_from(&page.open_cursor);
    let rooms: BTreeSet<_> = scopes.iter().map(|s| s.room).collect();
    mark.observed.retain(|room, prs| {
        // Missing from a paginated scan is not proof of closure.
        prs.retain(|id, _| !page.merged.iter().any(|pr| pr.number == *id));
        rooms.contains(room)
    });
    notices
}

fn merge_changes(
    repo: &str,
    scope: &Scope,
    scopes: &[Scope],
    page: &Page,
    fresh: &[&PullRequest],
    now: u64,
) -> Vec<Notice> {
    let mut notices = Vec::new();
    let mut other = Vec::new();
    for pr in fresh {
        if owns(scope, scopes, page, pr) {
            notices.push(notice(
                scope.room,
                repo,
                format!(
                    "#{} merged into {} at {} by {} (head {}). Next: cleanup / verify / follow-up.",
                    pr.number,
                    pr.base,
                    merge_time(pr.merged_at_ms.unwrap_or(now), now),
                    pr.merged_by,
                    pr.branch
                ),
                true,
            ));
        } else {
            other.push(format!("#{}", pr.number));
        }
    }
    if !other.is_empty() {
        notices.push(notice(
            scope.room,
            repo,
            format!(
                "{} merged into {} (informational).",
                other.join(", "),
                page.default_branch
            ),
            false,
        ));
    }
    notices
}

fn open_changes(
    repo: &str,
    scope: &Scope,
    scopes: &[Scope],
    page: &Page,
    observed: &mut std::collections::BTreeMap<u64, super::PrStatus>,
) -> Vec<Notice> {
    let mut notices = Vec::new();
    for pr in page.open.iter().filter(|pr| owns(scope, scopes, page, pr)) {
        let Some(old) = observed.get(&pr.number) else {
            observed.insert(pr.number, pr.status.clone());
            continue;
        };
        if pr.status.ready && !old.ready {
            notices.push(notice(
                scope.room,
                repo,
                format!(
                    "#{} ready for review (head {}). Next: assign review / verify.",
                    pr.number, pr.branch
                ),
                true,
            ));
        }
        if let Some(ci) = &pr.status.ci {
            if ci != "pending" && old.ci.as_ref() != Some(ci) {
                notices.push(notice(
                    scope.room,
                    repo,
                    format!(
                        "#{} CI completed: {} (head {}). Next: review / fix / merge follow-up.",
                        pr.number,
                        ci.split(':').next().unwrap_or(ci),
                        pr.branch
                    ),
                    true,
                ));
            }
        }
        if pr.mergeable_known && pr.status.conflict && !old.conflict {
            notices.push(notice(
                scope.room,
                repo,
                format!(
                    "#{} merge conflict appeared (head {}). Next: resolve / rebase.",
                    pr.number, pr.branch
                ),
                true,
            ));
        }
        let mut status = pr.status.clone();
        if !pr.mergeable_known {
            status.conflict = old.conflict;
        }
        if status.head == old.head && status.ci.is_none() {
            status.ci.clone_from(&old.ci);
        }
        observed.insert(pr.number, status);
    }
    notices
}
