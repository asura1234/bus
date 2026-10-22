use super::{
    discovery::{self, Interest},
    process,
    source::{self, Failure, Query},
    Page, Scope, Watermark, POLL_MS,
};
use std::{
    collections::BTreeMap,
    sync::{mpsc, Arc},
};

pub(super) trait Source: Send + Sync {
    fn discover(&self, interests: &[Interest]) -> BTreeMap<String, Vec<Scope>>;
    fn fetch(&self, query: &Query) -> Result<Page, Failure>;
}
struct GitHub {
    scratch: Option<std::path::PathBuf>,
}
impl Source for GitHub {
    fn discover(&self, interests: &[Interest]) -> BTreeMap<String, Vec<Scope>> {
        // Standard TUI tests never use real GitHub. An explicit marker in
        // the isolated session permits the read-only acceptance check.
        if self
            .scratch
            .as_ref()
            .is_some_and(|dir| !dir.join("github-watch-live").is_file())
        {
            return BTreeMap::new();
        }
        discovery::discover(interests)
    }
    fn fetch(&self, q: &Query) -> Result<Page, Failure> {
        let args = vec![
            "api".into(),
            "--hostname".into(),
            "github.com".into(),
            "graphql".into(),
            "--include".into(),
            "-f".into(),
            format!("query={}", source::query(q)),
        ];
        let (success, output, stderr) = process::output("gh", &args).map_err(|reason| Failure {
            reason,
            retry_at_ms: None,
        })?;
        let (headers, body) = split_headers(&output);
        let retry = retry_at(headers, q.now_ms);
        if !success {
            return Err(Failure {
                reason: if stderr.contains("gh auth login") || stderr.contains("GH_TOKEN") {
                    "GitHub watch requires gh authentication; run gh auth login"
                } else {
                    "GitHub request failed"
                },
                retry_at_ms: retry,
            });
        }
        let value = serde_json::from_str(body).map_err(|_| Failure {
            reason: "Invalid GitHub response",
            retry_at_ms: retry,
        })?;
        source::parse(q, &value).map(|mut page| {
            page.retry_at_ms = page.retry_at_ms.max(retry);
            page
        })
    }
}
fn split_headers(output: &str) -> (&str, &str) {
    if output.starts_with("HTTP/") {
        if let Some((headers, body)) = output
            .split_once("\r\n\r\n")
            .or_else(|| output.split_once("\n\n"))
        {
            return (headers, body);
        }
    }
    ("", output)
}
fn retry_at(headers: &str, now: u64) -> Option<u64> {
    let headers: BTreeMap<_, _> = headers
        .lines()
        .filter_map(|s| s.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
        .collect();
    let secondary = headers
        .get("retry-after")
        .and_then(|s| s.parse::<u64>().ok())
        .map(|seconds| now.saturating_add(seconds.saturating_mul(1000)));
    let primary = if headers.get("x-ratelimit-remaining") == Some(&"0") {
        headers
            .get("x-ratelimit-reset")
            .and_then(|s| s.parse::<u64>().ok())
            .map(|t| t.saturating_mul(1000))
    } else {
        None
    };
    secondary.max(primary)
}

#[derive(Clone, Debug)]
pub(crate) enum Update {
    Coverage(BTreeMap<String, Vec<Scope>>),
    Repo {
        repo: String,
        scopes: Vec<Scope>,
        result: Result<Page, Failure>,
        at_ms: u64,
    },
}
enum Message {
    Update(Update),
    Done,
}
#[derive(Clone, Default)]
struct Schedule {
    next: u64,
    failures: u32,
}

/// One background round; neither git nor gh can block the coordinator. Tests
/// inject a source; the default test runtime never invokes installed tools.
pub(crate) struct Runtime {
    source: Option<Arc<dyn Source>>,
    pending: Option<mpsc::Receiver<Message>>,
    next_round: u64,
    schedules: BTreeMap<String, Schedule>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            source: if cfg!(test) {
                None
            } else {
                Some(Arc::new(GitHub { scratch: None }))
            },
            pending: None,
            next_round: 0,
            schedules: BTreeMap::new(),
        }
    }
}
impl Runtime {
    pub(crate) fn for_session(data_dir: &std::path::Path) -> Self {
        let mut runtime = Self::default();
        if !cfg!(test) && cfg!(feature = "dev-tools") && std::env::var_os("BUS_TUI_OWNER").is_some()
        {
            runtime.source = Some(Arc::new(GitHub {
                scratch: Some(data_dir.to_path_buf()),
            }));
        }
        runtime
    }
    #[cfg(test)]
    pub(super) fn with_source(source: Arc<dyn Source>) -> Self {
        Self {
            source: Some(source),
            ..Self::default()
        }
    }
    pub(crate) fn needs_interests(&self, now: u64) -> bool {
        self.source.is_some() && now >= self.next_round
    }
    pub(crate) fn tick(
        &mut self,
        interests: Vec<Interest>,
        marks: &BTreeMap<String, Watermark>,
        now: u64,
    ) -> Vec<Update> {
        let mut updates = Vec::new();
        if let Some(receiver) = &self.pending {
            let mut done = false;
            loop {
                match receiver.try_recv() {
                    Ok(Message::Update(update)) => updates.push(update),
                    Ok(Message::Done) | Err(mpsc::TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            if done {
                self.pending = None;
            }
        }
        for update in &updates {
            if let Update::Repo {
                repo,
                result,
                at_ms,
                ..
            } = update
            {
                let schedule = self.schedules.entry(repo.clone()).or_default();
                schedule.next = at_ms.saturating_add(POLL_MS);
                match result {
                    Ok(page) => {
                        schedule.failures = 0;
                        schedule.next = schedule.next.max(page.retry_at_ms.unwrap_or(0));
                    }
                    Err(failure) => {
                        schedule.failures = schedule.failures.saturating_add(1);
                        let delay = POLL_MS.saturating_mul(1 << schedule.failures.min(4));
                        schedule.next = at_ms
                            .saturating_add(delay)
                            .max(failure.retry_at_ms.unwrap_or(0));
                        tracing::warn!(
                            event = "bus.github.unavailable",
                            repo,
                            reason = failure.reason,
                            "GitHub watch will retry"
                        );
                    }
                }
            }
        }
        if self.pending.is_none() && now >= self.next_round && updates.is_empty() {
            self.start(interests, marks.clone(), now);
        }
        updates
    }
    fn start(&mut self, interests: Vec<Interest>, marks: BTreeMap<String, Watermark>, now: u64) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let schedules = self.schedules.clone();
        let (tx, rx) = mpsc::channel();
        self.next_round = now.saturating_add(POLL_MS);
        let spawned = std::thread::Builder::new()
            .name("bus-github-watch".into())
            .spawn(move || {
                let coverage = source.discover(&interests);
                if tx
                    .send(Message::Update(Update::Coverage(coverage.clone())))
                    .is_err()
                {
                    return;
                }
                for (repo, scopes) in coverage {
                    if schedules.get(&repo).is_some_and(|s| s.next > now) {
                        continue;
                    }
                    let q = Query {
                        repo: repo.clone(),
                        branches: scopes.iter().flat_map(|s| s.branches.clone()).collect(),
                        watermark: marks.get(&repo).cloned().unwrap_or_default(),
                        now_ms: now,
                    };
                    let result = source.fetch(&q);
                    if tx
                        .send(Message::Update(Update::Repo {
                            repo,
                            scopes,
                            result,
                            at_ms: now,
                        }))
                        .is_err()
                    {
                        return;
                    }
                }
                let _ = tx.send(Message::Done);
            });
        if spawned.is_ok() {
            self.pending = Some(rx);
        } else {
            tracing::warn!(
                event = "bus.github.unavailable",
                "Could not start GitHub watch thread"
            );
        }
    }
}

#[cfg(test)]
#[path = "tests/runtime_test.rs"]
mod tests;
