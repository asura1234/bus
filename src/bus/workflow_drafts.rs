use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::bus::{
    model::RoomId,
    orchestrator::{
        ApprovedWorkflowPromotion, CreateDeveloperWorkflowApproval, ParticipantId,
        WorkflowDraftMutation,
    },
};

const MAX_SOP_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WorkflowDraftRecord {
    pub(crate) room_id: RoomId,
    pub(crate) draft_id: String,
    pub(crate) revision: u64,
    pub(crate) content_digest: String,
    #[serde(default)]
    pub(crate) workflow_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DeveloperWorkflowApproval {
    pub(crate) approval_id: String,
    pub(crate) developer: ParticipantId,
    pub(crate) room_id: RoomId,
    pub(crate) draft_id: String,
    pub(crate) draft_revision: u64,
    pub(crate) content_digest: String,
    pub(crate) workflow_id: String,
    pub(crate) standard_base_digest: Option<String>,
    pub(crate) reviewed_diff_digest: String,
    pub(crate) consumed: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WorkflowDraftLedger {
    #[serde(default)]
    drafts: BTreeMap<String, WorkflowDraftRecord>,
    #[serde(default)]
    approvals: BTreeMap<String, DeveloperWorkflowApproval>,
    #[serde(default = "initial_id")]
    next_id: u64,
}

impl WorkflowDraftLedger {
    #[cfg(test)]
    pub(crate) fn approval(&self, approval_id: &str) -> Option<&DeveloperWorkflowApproval> {
        self.approvals.get(approval_id)
    }

    pub(crate) fn approvals(&self) -> impl Iterator<Item = &DeveloperWorkflowApproval> {
        self.approvals.values()
    }
}

fn initial_id() -> u64 {
    1
}

fn draft_key(room_id: RoomId, draft_id: &str) -> String {
    format!("{}:{draft_id}", room_id.0)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowDraftReceipt {
    pub(crate) draft_id: String,
    pub(crate) revision: u64,
    pub(crate) content_digest: String,
    pub(crate) derived_target: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowPromotionReceipt {
    pub(crate) approval_id: String,
    pub(crate) content_digest: String,
    pub(crate) derived_target: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkflowPromotionReview {
    pub(crate) room_id: RoomId,
    pub(crate) expected_developer: ParticipantId,
    pub(crate) draft_id: String,
    pub(crate) draft_revision: u64,
    pub(crate) content_digest: String,
    pub(crate) workflow_id: String,
    pub(crate) standard_base_digest: Option<String>,
    pub(crate) rendered_diff: String,
    pub(crate) reviewed_diff_digest: String,
}

impl WorkflowPromotionReview {
    pub(crate) fn approval_command(&self) -> CreateDeveloperWorkflowApproval {
        CreateDeveloperWorkflowApproval {
            room_id: self.room_id,
            expected_developer: self.expected_developer.clone(),
            draft_id: self.draft_id.clone(),
            draft_revision: self.draft_revision,
            content_digest: self.content_digest.clone(),
            workflow_id: self.workflow_id.clone(),
            standard_base_digest: self.standard_base_digest.clone(),
            reviewed_diff_digest: self.reviewed_diff_digest.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowDraftError {
    InvalidRoot,
    InvalidIdentifier,
    InvalidSop,
    TooLarge,
    UnknownDraft,
    StaleRevision,
    TargetImmutable,
    Io,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WorkflowPromotionError {
    HumanAuthorityRequired,
    InvalidWorkflowId,
    NoPromotionTarget,
    UnknownApproval,
    ConsumedApproval,
    StaleApproval,
    UnsafeTarget,
    Io,
}

pub(crate) struct WorkflowDraftStore {
    repository_root: PathBuf,
}

impl WorkflowDraftStore {
    pub(crate) fn new(repository_root: PathBuf) -> Result<Self, WorkflowDraftError> {
        let repository_root = repository_root
            .canonicalize()
            .map_err(|_| WorkflowDraftError::InvalidRoot)?;
        if !repository_root.is_dir() {
            return Err(WorkflowDraftError::InvalidRoot);
        }
        Ok(Self { repository_root })
    }

    pub(crate) fn persist(
        &self,
        ledger: &mut WorkflowDraftLedger,
        room_id: RoomId,
        mutation: WorkflowDraftMutation,
    ) -> Result<WorkflowDraftReceipt, WorkflowDraftError> {
        validate_sop(&mutation.markdown)?;
        if let Some(workflow_id) = &mutation.workflow_id {
            validate_opaque_id(workflow_id)?;
        }
        let (draft_id, revision, workflow_id) = match mutation.draft_id {
            None => {
                if mutation.expected_revision.is_some() {
                    return Err(WorkflowDraftError::StaleRevision);
                }
                let id = ledger.next_id.max(1);
                ledger.next_id = id.saturating_add(1);
                (format!("draft-{id:016x}"), 1, mutation.workflow_id)
            }
            Some(draft_id) => {
                validate_opaque_id(&draft_id)?;
                let current = ledger
                    .drafts
                    .get(&draft_key(room_id, &draft_id))
                    .ok_or(WorkflowDraftError::UnknownDraft)?;
                if mutation.expected_revision != Some(current.revision) {
                    return Err(WorkflowDraftError::StaleRevision);
                }
                // The standard target is bound at creation so a reviewed diff cannot be retargeted.
                if mutation.workflow_id.is_some() && mutation.workflow_id != current.workflow_id {
                    return Err(WorkflowDraftError::TargetImmutable);
                }
                (
                    draft_id,
                    current.revision.saturating_add(1),
                    current.workflow_id.clone(),
                )
            }
        };
        let relative = PathBuf::from(".bus")
            .join("temp")
            .join(room_id.0.to_string())
            .join(format!("{draft_id}.md"));
        self.atomic_write_confined(&relative, mutation.markdown.as_bytes())
            .map_err(|_| WorkflowDraftError::Io)?;
        let content_digest = digest(mutation.markdown.as_bytes());
        ledger.drafts.insert(
            draft_key(room_id, &draft_id),
            WorkflowDraftRecord {
                room_id,
                draft_id: draft_id.clone(),
                revision,
                content_digest: content_digest.clone(),
                workflow_id,
            },
        );
        Ok(WorkflowDraftReceipt {
            draft_id,
            revision,
            content_digest,
            derived_target: display_relative(&relative),
        })
    }

    pub(crate) fn read(
        &self,
        ledger: &WorkflowDraftLedger,
        room_id: RoomId,
        draft_id: &str,
        max_bytes: u32,
    ) -> Result<(WorkflowDraftRecord, String), WorkflowDraftError> {
        validate_opaque_id(draft_id)?;
        if max_bytes == 0 || max_bytes as usize > MAX_SOP_BYTES {
            return Err(WorkflowDraftError::TooLarge);
        }
        let record = ledger
            .drafts
            .get(&draft_key(room_id, draft_id))
            .cloned()
            .ok_or(WorkflowDraftError::UnknownDraft)?;
        let relative = PathBuf::from(".bus")
            .join("temp")
            .join(room_id.0.to_string())
            .join(format!("{draft_id}.md"));
        let bytes =
            fs::read(self.repository_root.join(relative)).map_err(|_| WorkflowDraftError::Io)?;
        if bytes.len() > max_bytes as usize || digest(&bytes) != record.content_digest {
            return Err(WorkflowDraftError::TooLarge);
        }
        let markdown = String::from_utf8(bytes).map_err(|_| WorkflowDraftError::InvalidSop)?;
        Ok((record, markdown))
    }

    /// The single authoritative review computation: Human review surfaces display it and
    /// `create_approval` recomputes it, so a displayed command is exactly what validation accepts.
    pub(super) fn promotion_review(
        &self,
        ledger: &WorkflowDraftLedger,
        room_id: RoomId,
        draft_id: &str,
    ) -> Result<WorkflowPromotionReview, WorkflowPromotionError> {
        let draft = ledger
            .drafts
            .get(&draft_key(room_id, draft_id))
            .ok_or(WorkflowPromotionError::StaleApproval)?;
        let workflow_id = draft
            .workflow_id
            .as_deref()
            .ok_or(WorkflowPromotionError::NoPromotionTarget)?;
        validate_workflow_id(workflow_id)?;
        let draft_relative = PathBuf::from(".bus")
            .join("temp")
            .join(room_id.0.to_string())
            .join(format!("{draft_id}.md"));
        let draft_bytes = read_confined_regular(&self.repository_root, &draft_relative)?
            .ok_or(WorkflowPromotionError::StaleApproval)?;
        if digest(&draft_bytes) != draft.content_digest {
            return Err(WorkflowPromotionError::StaleApproval);
        }
        let standard_relative = PathBuf::from(".bus")
            .join("standard")
            .join(format!("{workflow_id}.md"));
        let standard_bytes = read_confined_regular(&self.repository_root, &standard_relative)?;
        let rendered_diff = render_full_replacement_diff(
            &display_relative(&standard_relative),
            standard_bytes.as_deref(),
            &draft_bytes,
        )?;
        Ok(WorkflowPromotionReview {
            room_id,
            expected_developer: ParticipantId::Human,
            draft_id: draft.draft_id.clone(),
            draft_revision: draft.revision,
            content_digest: draft.content_digest.clone(),
            workflow_id: workflow_id.to_owned(),
            standard_base_digest: standard_bytes.as_deref().map(digest),
            reviewed_diff_digest: digest(rendered_diff.as_bytes()),
            rendered_diff,
        })
    }

    pub(crate) fn promotion_reviews(
        &self,
        ledger: &WorkflowDraftLedger,
    ) -> BTreeMap<RoomId, Vec<WorkflowPromotionReview>> {
        let mut reviews = BTreeMap::<RoomId, Vec<WorkflowPromotionReview>>::new();
        for draft in ledger.drafts.values() {
            // Drafts without a target, or with unreadable confined evidence, have no reviewable diff.
            if let Ok(review) = self.promotion_review(ledger, draft.room_id, &draft.draft_id) {
                reviews.entry(draft.room_id).or_default().push(review);
            }
        }
        reviews
    }

    pub(super) fn create_approval(
        &self,
        ledger: &mut WorkflowDraftLedger,
        request: CreateDeveloperWorkflowApproval,
    ) -> Result<DeveloperWorkflowApproval, WorkflowPromotionError> {
        if request.expected_developer != ParticipantId::Human {
            return Err(WorkflowPromotionError::HumanAuthorityRequired);
        }
        if let Some(existing) = ledger.approvals.values().find(|approval| {
            approval.developer == request.expected_developer
                && approval.room_id == request.room_id
                && approval.draft_id == request.draft_id
                && approval.draft_revision == request.draft_revision
                && approval.content_digest == request.content_digest
                && approval.workflow_id == request.workflow_id
                && approval.standard_base_digest == request.standard_base_digest
                && approval.reviewed_diff_digest == request.reviewed_diff_digest
        }) {
            return Ok(existing.clone());
        }
        let authoritative = self
            .promotion_review(ledger, request.room_id, &request.draft_id)
            .map_err(|error| match error {
                WorkflowPromotionError::NoPromotionTarget => WorkflowPromotionError::StaleApproval,
                other => other,
            })?
            .approval_command();
        if authoritative != request {
            return Err(WorkflowPromotionError::StaleApproval);
        }
        let approval_id = format!("approval-{:016x}", ledger.next_id.max(1));
        ledger.next_id = ledger.next_id.max(1).saturating_add(1);
        let approval = DeveloperWorkflowApproval {
            approval_id: approval_id.clone(),
            developer: request.expected_developer,
            room_id: request.room_id,
            draft_id: request.draft_id,
            draft_revision: request.draft_revision,
            content_digest: request.content_digest,
            workflow_id: request.workflow_id,
            standard_base_digest: request.standard_base_digest,
            reviewed_diff_digest: request.reviewed_diff_digest,
            consumed: false,
        };
        ledger.approvals.insert(approval_id, approval.clone());
        Ok(approval)
    }

    pub(crate) fn promote(
        &self,
        ledger: &mut WorkflowDraftLedger,
        room_id: RoomId,
        promotion: ApprovedWorkflowPromotion,
    ) -> Result<WorkflowPromotionReceipt, WorkflowPromotionError> {
        let approval = ledger
            .approvals
            .get(&promotion.approval_id)
            .cloned()
            .ok_or(WorkflowPromotionError::UnknownApproval)?;
        if approval.consumed {
            return Err(WorkflowPromotionError::ConsumedApproval);
        }
        if approval.room_id != room_id {
            return Err(WorkflowPromotionError::StaleApproval);
        }
        validate_workflow_id(&approval.workflow_id)?;
        let draft = ledger
            .drafts
            .get(&draft_key(room_id, &approval.draft_id))
            .filter(|draft| {
                draft.revision == approval.draft_revision
                    && draft.content_digest == approval.content_digest
                    && draft.workflow_id.as_deref() == Some(approval.workflow_id.as_str())
            })
            .ok_or(WorkflowPromotionError::StaleApproval)?;
        let draft_relative = PathBuf::from(".bus")
            .join("temp")
            .join(room_id.0.to_string())
            .join(format!("{}.md", approval.draft_id));
        let bytes = read_confined_regular(&self.repository_root, &draft_relative)?
            .ok_or(WorkflowPromotionError::StaleApproval)?;
        if digest(&bytes) != draft.content_digest {
            return Err(WorkflowPromotionError::StaleApproval);
        }
        let standard_relative = PathBuf::from(".bus")
            .join("standard")
            .join(format!("{}.md", approval.workflow_id));
        let current_base_bytes = read_confined_regular(&self.repository_root, &standard_relative)?;
        let current_base = current_base_bytes.as_deref().map(digest);
        if current_base != approval.standard_base_digest {
            return Err(WorkflowPromotionError::StaleApproval);
        }
        let reviewed_diff_digest = digest(
            render_full_replacement_diff(
                &display_relative(&standard_relative),
                current_base_bytes.as_deref(),
                &bytes,
            )?
            .as_bytes(),
        );
        if reviewed_diff_digest != approval.reviewed_diff_digest {
            return Err(WorkflowPromotionError::StaleApproval);
        }
        self.atomic_write_confined(&standard_relative, &bytes)
            .map_err(|_| WorkflowPromotionError::Io)?;
        ledger
            .approvals
            .get_mut(&promotion.approval_id)
            .expect("approval remains present")
            .consumed = true;
        Ok(WorkflowPromotionReceipt {
            approval_id: promotion.approval_id,
            content_digest: approval.content_digest,
            derived_target: display_relative(&standard_relative),
        })
    }

    fn atomic_write_confined(&self, relative: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "unsafe path",
            ));
        }
        let parent = self
            .repository_root
            .join(relative)
            .parent()
            .unwrap()
            .to_owned();
        create_confined_directories(&self.repository_root, &parent)?;
        let canonical_parent = parent.canonicalize()?;
        if !canonical_parent.starts_with(&self.repository_root) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "path escape",
            ));
        }
        let target = canonical_parent.join(relative.file_name().unwrap());
        if let Ok(metadata) = fs::symlink_metadata(&target) {
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unsafe target",
                ));
            }
        }
        let temporary = canonical_parent.join(format!(
            ".{}.tmp-{}",
            relative.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temporary, &target) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }
}

fn render_full_replacement_diff(
    target: &str,
    before: Option<&[u8]>,
    after: &[u8],
) -> Result<String, WorkflowPromotionError> {
    let before = before
        .map(|bytes| std::str::from_utf8(bytes).map_err(|_| WorkflowPromotionError::UnsafeTarget))
        .transpose()?
        .unwrap_or("");
    let after = std::str::from_utf8(after).map_err(|_| WorkflowPromotionError::StaleApproval)?;
    let mut rendered = format!("--- {target}\n+++ {target}\n@@ full replacement @@\n");
    append_diff_body(&mut rendered, '-', before);
    append_diff_body(&mut rendered, '+', after);
    Ok(rendered)
}

fn append_diff_body(output: &mut String, prefix: char, text: &str) {
    for line in text.split_inclusive('\n') {
        output.push(prefix);
        output.push_str(line);
        if !line.ends_with('\n') {
            output.push('\n');
            output.push_str("\\ No newline at end of file\n");
        }
    }
}

fn read_confined_regular(
    root: &Path,
    relative: &Path,
) -> Result<Option<Vec<u8>>, WorkflowPromotionError> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(WorkflowPromotionError::UnsafeTarget);
    }
    let mut current = root.to_owned();
    let mut components = relative.components().peekable();
    while let Some(std::path::Component::Normal(component)) = components.next() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if components.peek().is_some() => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(WorkflowPromotionError::UnsafeTarget);
                }
            }
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(WorkflowPromotionError::UnsafeTarget);
                }
                return fs::read(&current)
                    .map(Some)
                    .map_err(|_| WorkflowPromotionError::Io);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(WorkflowPromotionError::Io),
        }
    }
    Err(WorkflowPromotionError::UnsafeTarget)
}

fn create_confined_directories(root: &Path, target: &Path) -> std::io::Result<()> {
    let relative = target
        .strip_prefix(root)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::PermissionDenied, "path escape"))?;
    let mut current = root.to_owned();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "unsafe path",
            ));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unsafe directory",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&current)?,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn validate_sop(markdown: &str) -> Result<(), WorkflowDraftError> {
    if markdown.len() > MAX_SOP_BYTES {
        return Err(WorkflowDraftError::TooLarge);
    }
    if !markdown.starts_with("# ") || !markdown.lines().any(|line| line.starts_with("## ")) {
        return Err(WorkflowDraftError::InvalidSop);
    }
    Ok(())
}

fn validate_opaque_id(value: &str) -> Result<(), WorkflowDraftError> {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Ok(())
    } else {
        Err(WorkflowDraftError::InvalidIdentifier)
    }
}

fn validate_workflow_id(value: &str) -> Result<(), WorkflowPromotionError> {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        Ok(())
    } else {
        Err(WorkflowPromotionError::InvalidWorkflowId)
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests;
