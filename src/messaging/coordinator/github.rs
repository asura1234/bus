use super::worker::Worker;
impl Worker {
    pub(super) fn refresh_github(&mut self) -> Result<(), String> {
        let now = super::io::now_ms();
        if self.github_pending.is_empty() {
            let interests = if self.github.needs_interests(now) {
                self.state.github_interests()
            } else {
                Vec::new()
            };
            self.github_pending = self
                .github
                .tick(interests, self.state.github_watermarks(), now);
        }
        if self.github_pending.is_empty() {
            return Ok(());
        }
        let mut state = self.state.clone();
        for update in &self.github_pending {
            state.apply_github(update, now).map_err(|e| e.to_string())?;
        }
        // On failure retain results for retry; neither notices nor watermark
        // become acknowledged until the single state transaction commits.
        self.save(state)?;
        self.github_pending.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::{
        github::{Page, PullRequest, Scope, Update},
        model::BusState,
        native::{Transport, TransportError},
    };
    use crate::protocol::api::schema::{Method, ResponseResult};
    struct NoTransport;
    impl Transport for NoTransport {
        fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
            panic!("GitHub persistence must not use native transport")
        }
    }
    #[test]
    fn github_watch_failed_save_retains_result_and_does_not_advance_watermark() {
        let dir = std::env::temp_dir().join(format!(
            "bus-github-save-{}-{}",
            std::process::id(),
            super::super::io::now_ns()
        ));
        let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
        let room = worker.state.create_room("watch").unwrap();
        worker.save(worker.state.clone()).unwrap();
        let page = Page {
            default_branch: "main".into(),
            merged: vec![PullRequest {
                number: 137,
                base: "main".into(),
                merged_at_ms: Some(1000),
                ..Default::default()
            }],
            ..Default::default()
        };
        worker.github_pending.push(Update::Repo {
            repo: "o/r".into(),
            scopes: vec![Scope {
                room: room.0,
                ..Default::default()
            }],
            result: Ok(page),
            at_ms: 2000,
        });
        let path = worker.store.path().to_path_buf();
        let saved = dir.join("saved.json");
        std::fs::rename(&path, &saved).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(worker.refresh_github().is_err());
        assert_eq!(worker.github_pending.len(), 1);
        assert!(worker.state.github_watermarks().is_empty());
        assert_eq!(worker.state.notices(room).count(), 0);
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&saved, &path).unwrap();
        worker.refresh_github().unwrap();
        assert!(worker.github_pending.is_empty());
        assert_eq!(worker.state.notices(room).count(), 1);
        let loaded: BusState = worker.store.load().unwrap().unwrap();
        assert_eq!(loaded.github_watermarks(), worker.state.github_watermarks());
        assert_eq!(loaded.notices(room).count(), 1);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
