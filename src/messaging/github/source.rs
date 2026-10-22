//! A single GraphQL request observes open PRs and default-branch merges.
use super::{Page, PrStatus, PullRequest, Watermark, LOOKBACK_MS, OVERLAP_MS};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub(crate) struct Query {
    pub(crate) repo: String,
    pub(crate) watermark: Watermark,
    pub(crate) now_ms: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub(crate) reason: &'static str,
    pub(crate) retry_at_ms: Option<u64>,
}

pub(super) fn query(q: &Query) -> String {
    let (owner, name) = q.repo.split_once('/').expect("validated repository");
    format!(
        r#"query {{
      rateLimit {{ remaining resetAt cost }}
      repository(owner: {owner}, name: {name}) {{
        defaultBranchRef {{ name }}
        open: pullRequests(states: OPEN, first: 50, after: {open}, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
          pageInfo {{ hasNextPage endCursor }}
          nodes {{ {fields} isDraft mergeable headRefOid
            commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state
              contexts(first: 100) {{ totalCount
                checkRunCountsByState {{ state count }} statusContextCountsByState {{ state count }}
                pageInfo {{ hasNextPage }} nodes {{
                ... on CheckRun {{ id status conclusion completedAt }}
                ... on StatusContext {{ id state createdAt }}
              }} }}
            }} }} }} }}
          }}
        }}
        merged: pullRequests(states: [MERGED, CLOSED], first: 50, after: {merged}, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{
          pageInfo {{ hasNextPage endCursor }}
          nodes {{ {fields} state mergedAt mergedBy {{ login }} updatedAt }}
        }}
      }}
    }}"#,
        owner = json!(owner),
        name = json!(name),
        open = json!(q.watermark.open_cursor),
        merged = json!(q.watermark.merged_cursor),
        fields = "number headRefName baseRefName"
    )
}

pub(super) fn parse(q: &Query, body: &Value) -> Result<Page, Failure> {
    let retry = body.pointer("/data/rateLimit/resetAt").and_then(timestamp);
    if body.get("errors").is_some() {
        return Err(Failure {
            reason: "GitHub GraphQL request failed",
            retry_at_ms: retry,
        });
    }
    let repo = body
        .pointer("/data/repository")
        .filter(|v| v.is_object())
        .ok_or(Failure {
            reason: "GitHub repository unavailable",
            retry_at_ms: None,
        })?;
    let default_branch = repo
        .pointer("/defaultBranchRef/name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(Failure {
            reason: "GitHub default branch unavailable",
            retry_at_ms: None,
        })?
        .into();
    let open = nodes(repo, "open")?
        .iter()
        .map(pull_request)
        .collect::<Result<_, _>>()?;
    let merged_nodes = nodes(repo, "merged")?;
    let since = if q.watermark.merged_since_ms == 0 {
        if q.watermark.scan_started_ms == 0 {
            q.now_ms
        } else {
            q.watermark.scan_started_ms
        }
        .saturating_sub(LOOKBACK_MS)
    } else {
        q.watermark.merged_since_ms.saturating_sub(OVERLAP_MS)
    };
    // Merged and closed PRs share the durable UPDATED_AT scan cursor.
    let older = merged_nodes
        .last()
        .and_then(|n| n.get("updatedAt"))
        .and_then(timestamp)
        .is_some_and(|t| t < since);
    Ok(Page {
        default_branch,
        open,
        merged: merged_nodes
            .iter()
            .filter(|pr| pr["state"] == "MERGED")
            .map(pull_request)
            .collect::<Result<_, _>>()?,
        closed: merged_nodes
            .iter()
            .filter(|pr| pr["state"] == "CLOSED")
            .map(pull_request)
            .collect::<Result<_, _>>()?,
        open_cursor: cursor(repo, "open")?,
        merged_cursor: if older { None } else { cursor(repo, "merged")? },
        retry_at_ms: (body
            .pointer("/data/rateLimit/remaining")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            < 100)
            .then_some(retry)
            .flatten(),
    })
}

fn nodes<'a>(repo: &'a Value, connection: &str) -> Result<&'a Vec<Value>, Failure> {
    repo.get(connection)
        .and_then(|v| v.get("nodes"))
        .and_then(Value::as_array)
        .ok_or(Failure {
            reason: "Incomplete GitHub response",
            retry_at_ms: None,
        })
}
fn cursor(repo: &Value, connection: &str) -> Result<Option<String>, Failure> {
    let info = &repo[connection]["pageInfo"];
    match info["hasNextPage"].as_bool() {
        Some(false) => Ok(None),
        Some(true) => info["endCursor"]
            .as_str()
            .map(|s| Some(s.into()))
            .ok_or(Failure {
                reason: "Missing GitHub page cursor",
                retry_at_ms: None,
            }),
        None => Err(Failure {
            reason: "Missing GitHub page information",
            retry_at_ms: None,
        }),
    }
}
fn pull_request(v: &Value) -> Result<PullRequest, Failure> {
    let string = |field: &str| v[field].as_str().unwrap_or("").to_owned();
    Ok(PullRequest {
        number: v["number"].as_u64().ok_or(Failure {
            reason: "Invalid GitHub PR",
            retry_at_ms: None,
        })?,
        branch: string("headRefName"),
        base: string("baseRefName"),
        merged_by: v["mergedBy"]["login"].as_str().unwrap_or("unknown").into(),
        merged_at_ms: v.get("mergedAt").and_then(timestamp),
        mergeable_known: matches!(v["mergeable"].as_str(), Some("MERGEABLE" | "CONFLICTING")),
        status: PrStatus {
            head: string("headRefOid"),
            ready: v["isDraft"] == false,
            conflict: v["mergeable"] == "CONFLICTING",
            ci: ci(v),
        },
    })
}
fn ci(v: &Value) -> Option<String> {
    let rollup = v.pointer("/commits/nodes/0/commit/statusCheckRollup")?;
    let result = match rollup["state"].as_str()? {
        "SUCCESS" => "success",
        "FAILURE" | "ERROR" => "failure",
        "PENDING" => return Some("pending".into()),
        _ => return None,
    };
    let connection = &rollup["contexts"];
    let contexts = connection["nodes"].as_array()?;
    if contexts.is_empty() {
        return None;
    }
    // Aggregate state counts cover ALL contexts, including those beyond the
    // first page. A failure alone does not imply the other jobs finished.
    let running = if connection["pageInfo"]["hasNextPage"].as_bool()? {
        let checks = connection["checkRunCountsByState"].as_array()?;
        let statuses = connection["statusContextCountsByState"].as_array()?;
        checks.iter().chain(statuses).any(|c| {
            c["count"].as_u64().unwrap_or(0) > 0
                && matches!(
                    c["state"].as_str(),
                    Some(
                        "PENDING" | "EXPECTED" | "QUEUED" | "WAITING" | "IN_PROGRESS" | "REQUESTED"
                    )
                )
        })
    } else {
        contexts.iter().any(|c| {
            c.get("status").is_some_and(|s| s != "COMPLETED")
                || matches!(c["state"].as_str(), Some("PENDING" | "EXPECTED"))
        })
    };
    if running {
        return Some("pending".into());
    }
    let mut ids: Vec<_> = contexts.iter().map(Value::to_string).collect();
    ids.sort();
    Some(format!(
        "{result}:{}:{}:{}:{}:{}",
        v["headRefOid"].as_str()?,
        connection["totalCount"],
        connection["checkRunCountsByState"],
        connection["statusContextCountsByState"],
        ids.join("|")
    ))
}
fn timestamp(v: &Value) -> Option<u64> {
    let time =
        time::OffsetDateTime::parse(v.as_str()?, &time::format_description::well_known::Rfc3339)
            .ok()?;
    u64::try_from(time.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
#[path = "tests/source_test.rs"]
mod tests;
