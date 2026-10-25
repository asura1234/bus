use std::time::Instant;

use bytes::Bytes;

use super::{
    ActiveSubmission, ActorState, PendingWrite, PtyIoActorRunner, PtyIoDataCommand,
    SubmissionBoundary, SubmissionPhase, ACTOR_IDLE_POLL_MS,
};

impl PtyIoActorRunner {
    pub(super) fn enqueue_submission_write(&mut self, bytes: Bytes, boundary: SubmissionBoundary) {
        if !bytes.is_empty() {
            self.pending_writes.push_back(PendingWrite {
                bytes,
                boundary: Some(boundary),
            });
        }
    }

    pub(super) fn handle_data_command(&mut self, command: PtyIoDataCommand) -> bool {
        match command {
            PtyIoDataCommand::WriteUserInput(bytes) => {
                if self.state == ActorState::Running {
                    self.enqueue_write(bytes);
                }
            }
            PtyIoDataCommand::SubmitUserInput {
                text,
                enter,
                delay,
                mut permit,
                reply,
            } => {
                if self.state == ActorState::Running {
                    if !text.is_empty() && !enter.is_empty() {
                        if let Some(deadline) = permit.deadline {
                            if deadline.saturating_duration_since(Instant::now()) <= delay {
                                let _ = reply.send(Err(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "PTY input submission cannot fit its Enter delay",
                                )));
                                return false;
                            }
                            // Reserve the Enter delay while the text is still
                            // queued. The first written byte commits both parts.
                            permit.deadline = deadline.checked_sub(delay);
                        }
                    }
                    let phase = if text.is_empty() {
                        SubmissionPhase::WaitingUntil(Instant::now() + delay)
                    } else {
                        self.enqueue_submission_write(text, SubmissionBoundary::Text);
                        SubmissionPhase::WritingText
                    };
                    self.active_submission = Some(ActiveSubmission {
                        permit,
                        enter,
                        delay,
                        phase,
                        reply,
                    });
                } else {
                    let _ = reply.send(Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "pty actor is not accepting input",
                    )));
                }
            }
        }
        false
    }

    pub(super) fn commit_submission_text(&mut self) {
        if let Some(submission) = self
            .active_submission
            .as_mut()
            .filter(|s| !s.enter.is_empty())
        {
            // A combined legacy submission must finish text + Enter once text
            // starts, matching Windows. Tracked waiter cancellation still uses
            // the shared flag; observed single-segment writes never commit here.
            submission.permit.deadline = None;
        }
    }

    pub(super) fn complete_submission_boundary(&mut self, boundary: SubmissionBoundary) {
        match boundary {
            SubmissionBoundary::Text => {
                let Some(submission) = self.active_submission.as_mut() else {
                    return;
                };
                debug_assert!(matches!(submission.phase, SubmissionPhase::WritingText));
                submission.phase = SubmissionPhase::WaitingUntil(Instant::now() + submission.delay);
            }
            SubmissionBoundary::Enter => {
                let Some(submission) = self.active_submission.take() else {
                    return;
                };
                debug_assert!(matches!(submission.phase, SubmissionPhase::WritingEnter));
                let _ = submission.reply.send(Ok(()));
            }
        }
    }

    pub(super) fn schedule_submission_enter(&mut self) {
        self.expire_submission();
        let Some(ActiveSubmission {
            enter,
            phase: SubmissionPhase::WaitingUntil(deadline),
            ..
        }) = self.active_submission.as_ref()
        else {
            return;
        };
        if Instant::now() >= *deadline {
            let enter = enter.clone();
            if enter.is_empty() {
                // The waiting submission above cannot disappear: this actor
                // exclusively owns it and has not processed another command.
                if let Some(submission) = self.active_submission.take() {
                    let _ = submission.reply.send(Ok(()));
                }
            } else {
                // The same exclusive actor ownership keeps the waiting value present.
                if let Some(submission) = self.active_submission.as_mut() {
                    submission.phase = SubmissionPhase::WritingEnter;
                }
                self.enqueue_submission_write(enter, SubmissionBoundary::Enter);
            }
        }
    }

    pub(super) fn poll_timeout_ms(&self) -> i32 {
        let Some(submission) = self.active_submission.as_ref() else {
            return ACTOR_IDLE_POLL_MS;
        };
        let enter_at = match submission.phase {
            SubmissionPhase::WaitingUntil(at) => Some(at),
            _ => None,
        };
        enter_at
            .into_iter()
            .chain(submission.permit.deadline)
            .min()
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .max(1)
                    .min(ACTOR_IDLE_POLL_MS as u128) as i32
            })
            .unwrap_or(ACTOR_IDLE_POLL_MS)
    }

    pub(super) fn expire_submission(&mut self) {
        let expired = self
            .active_submission
            .as_ref()
            .and_then(|submission| submission.permit.lock().err());
        if let Some(error) = expired {
            self.fail_active_submission(error);
        }
    }

    pub(super) fn fail_active_submission(&mut self, err: std::io::Error) {
        if let Some(submission) = self.active_submission.take() {
            if self
                .pending_writes
                .front()
                .is_some_and(|write| write.boundary.is_some())
            {
                self.current_write_offset = 0;
            }
            self.pending_writes.retain(|write| write.boundary.is_none());
            let _ = submission.reply.send(Err(err));
        }
    }

    pub(super) fn close_input_queue(&mut self) {
        self.data_rx.close();
        self.fail_active_submission(input_submission_closed_error());
        while let Some(command) = self.data_rx.blocking_recv() {
            if let PtyIoDataCommand::SubmitUserInput { reply, .. } = command {
                let _ = reply.send(Err(input_submission_closed_error()));
            }
        }
    }
}

fn input_submission_closed_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "PTY actor closed during input submission",
    )
}
