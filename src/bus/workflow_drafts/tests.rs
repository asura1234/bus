use super::*;
use crate::bus::{
    model::RoomId,
    orchestrator::{ApprovedWorkflowPromotion, ParticipantId, WorkflowDraftMutation},
};

fn temp_repo() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "bus-workflow-drafts-{}-{}",
        std::process::id(),
        crate::bus::io::now_ns()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn mutation(
    draft_id: Option<&str>,
    expected_revision: Option<u64>,
    workflow_id: Option<&str>,
    markdown: &str,
) -> WorkflowDraftMutation {
    WorkflowDraftMutation {
        draft_id: draft_id.map(Into::into),
        expected_revision,
        workflow_id: workflow_id.map(Into::into),
        markdown: markdown.into(),
    }
}

#[test]
fn room_orchestrator_core_workflow_draft_is_pathless_bounded_and_revisioned() {
    let root = temp_repo();
    let source = root.join("src.rs");
    std::fs::write(&source, b"unchanged").unwrap();
    let store = WorkflowDraftStore::new(root.clone()).unwrap();
    let mut ledger = WorkflowDraftLedger::default();
    let created = store
        .persist(
            &mut ledger,
            RoomId(7),
            mutation(
                None,
                None,
                None,
                "# Recovery SOP\n\n## Observe\n\nInspect facts.\n",
            ),
        )
        .unwrap();
    assert_eq!(created.revision, 1);
    assert!(created.derived_target.starts_with(".bus/temp/7/"));
    assert!(created.derived_target.ends_with(".md"));
    assert_eq!(std::fs::read(&source).unwrap(), b"unchanged");
    let stale = store.persist(
        &mut ledger,
        RoomId(7),
        mutation(
            Some(&created.draft_id),
            Some(0),
            None,
            "# Recovery SOP\n\n## Observe\n\nNew facts.\n",
        ),
    );
    assert!(matches!(stale, Err(WorkflowDraftError::StaleRevision)));
    let oversized = store.persist(
        &mut ledger,
        RoomId(7),
        mutation(
            None,
            None,
            None,
            &format!("# SOP\n\n## Step\n\n{}", "x".repeat(128 * 1024)),
        ),
    );
    assert!(matches!(oversized, Err(WorkflowDraftError::TooLarge)));
    assert_eq!(std::fs::read(&source).unwrap(), b"unchanged");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn room_orchestrator_core_standard_promotion_requires_exact_single_use_human_review() {
    let root = temp_repo();
    let store = WorkflowDraftStore::new(root.clone()).unwrap();
    let mut ledger = WorkflowDraftLedger::default();
    let draft = store
        .persist(
            &mut ledger,
            RoomId(2),
            mutation(
                None,
                None,
                Some("release-sop"),
                "# Release SOP\n\n## Verify\n\nRead evidence.\n",
            ),
        )
        .unwrap();
    assert!(matches!(
        store.promote(
            &mut ledger,
            RoomId(2),
            ApprovedWorkflowPromotion {
                approval_id: "forged".into()
            }
        ),
        Err(WorkflowPromotionError::UnknownApproval)
    ));
    let approval_request = store
        .promotion_review(&ledger, RoomId(2), &draft.draft_id)
        .unwrap()
        .approval_command();
    let mut forged_developer = approval_request.clone();
    forged_developer.expected_developer = ParticipantId::Orchestrator;
    assert!(matches!(
        store.create_approval(&mut ledger, forged_developer),
        Err(WorkflowPromotionError::HumanAuthorityRequired)
    ));
    let mut stale_review = approval_request.clone();
    stale_review.reviewed_diff_digest = "not-the-reviewed-diff".into();
    assert!(matches!(
        store.create_approval(&mut ledger, stale_review),
        Err(WorkflowPromotionError::StaleApproval)
    ));
    let approval = store
        .create_approval(&mut ledger, approval_request.clone())
        .unwrap();
    assert_eq!(
        store
            .create_approval(&mut ledger, approval_request)
            .unwrap(),
        approval
    );
    assert_eq!(ledger.approvals().count(), 1);
    let promoted = store
        .promote(
            &mut ledger,
            RoomId(2),
            ApprovedWorkflowPromotion {
                approval_id: approval.approval_id.clone(),
            },
        )
        .unwrap();
    assert_eq!(promoted.derived_target, ".bus/standard/release-sop.md");
    assert!(matches!(
        store.promote(
            &mut ledger,
            RoomId(2),
            ApprovedWorkflowPromotion {
                approval_id: approval.approval_id
            }
        ),
        Err(WorkflowPromotionError::ConsumedApproval)
    ));
    assert_eq!(
        std::fs::read_to_string(root.join(".bus/standard/release-sop.md")).unwrap(),
        "# Release SOP\n\n## Verify\n\nRead evidence.\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn room_orchestrator_core_workflow_promotion_review_is_the_exact_approval_recomputation() {
    let root = temp_repo();
    let store = WorkflowDraftStore::new(root.clone()).unwrap();
    let mut ledger = WorkflowDraftLedger::default();
    let room = RoomId(4);
    let local = store
        .persist(
            &mut ledger,
            room,
            mutation(None, None, None, "# Local SOP\n\n## Step\n\nRoom only.\n"),
        )
        .unwrap();
    assert_eq!(
        store.promotion_review(&ledger, room, &local.draft_id),
        Err(WorkflowPromotionError::NoPromotionTarget)
    );
    assert_eq!(
        store.persist(
            &mut ledger,
            room,
            mutation(
                None,
                None,
                Some("../escape"),
                "# Bad SOP\n\n## Step\n\nNo.\n"
            ),
        ),
        Err(WorkflowDraftError::InvalidIdentifier)
    );

    let draft = store
        .persist(
            &mut ledger,
            room,
            mutation(
                None,
                None,
                Some("release-sop"),
                "# Release SOP\n\n## Verify\n\nRead evidence.\n",
            ),
        )
        .unwrap();
    let review = store
        .promotion_review(&ledger, room, &draft.draft_id)
        .unwrap();
    assert_eq!(review.room_id, room);
    assert_eq!(review.expected_developer, ParticipantId::Human);
    assert_eq!(review.draft_id, draft.draft_id);
    assert_eq!(review.draft_revision, 1);
    assert_eq!(review.content_digest, draft.content_digest);
    assert_eq!(review.workflow_id, "release-sop");
    assert_eq!(review.standard_base_digest, None);
    assert_eq!(
        review.reviewed_diff_digest,
        digest(review.rendered_diff.as_bytes())
    );
    assert!(review
        .rendered_diff
        .starts_with("--- .bus/standard/release-sop.md\n"));

    assert_eq!(
        store.persist(
            &mut ledger,
            room,
            mutation(
                Some(&draft.draft_id),
                Some(1),
                Some("other-sop"),
                "# Release SOP\n\n## Verify\n\nMoved.\n",
            ),
        ),
        Err(WorkflowDraftError::TargetImmutable)
    );
    assert_eq!(
        store
            .promotion_review(&ledger, room, &draft.draft_id)
            .unwrap(),
        review
    );

    let command = review.approval_command();
    let tampers: Vec<fn(&mut CreateDeveloperWorkflowApproval)> = vec![
        |command| command.room_id = RoomId(5),
        |command| command.draft_id = "draft-ffffffffffffffff".into(),
        |command| command.draft_revision += 1,
        |command| command.content_digest = "other".into(),
        |command| command.workflow_id = "other-sop".into(),
        |command| command.standard_base_digest = Some("other".into()),
        |command| command.reviewed_diff_digest = "other".into(),
    ];
    for tamper in tampers {
        let mut forged = command.clone();
        tamper(&mut forged);
        assert_eq!(
            store.create_approval(&mut ledger, forged),
            Err(WorkflowPromotionError::StaleApproval)
        );
    }
    assert_eq!(ledger.approvals().count(), 0);

    store
        .persist(
            &mut ledger,
            room,
            mutation(
                Some(&draft.draft_id),
                Some(1),
                Some("release-sop"),
                "# Release SOP\n\n## Verify\n\nRead revised evidence.\n",
            ),
        )
        .unwrap();
    assert_eq!(
        store.create_approval(&mut ledger, command),
        Err(WorkflowPromotionError::StaleApproval)
    );
    let revised = store
        .promotion_review(&ledger, room, &draft.draft_id)
        .unwrap();
    assert_eq!(revised.draft_revision, 2);

    std::fs::create_dir_all(root.join(".bus/standard")).unwrap();
    std::fs::write(
        root.join(".bus/standard/release-sop.md"),
        "# Release SOP\n\n## Old\n",
    )
    .unwrap();
    assert_eq!(
        store.create_approval(&mut ledger, revised.approval_command()),
        Err(WorkflowPromotionError::StaleApproval)
    );
    let current = store
        .promotion_review(&ledger, room, &draft.draft_id)
        .unwrap();
    assert_eq!(
        current.standard_base_digest,
        Some(digest(b"# Release SOP\n\n## Old\n"))
    );

    let approval = store
        .create_approval(&mut ledger, current.approval_command())
        .unwrap();
    assert_eq!(
        (
            approval.developer,
            approval.room_id,
            approval.draft_id,
            approval.draft_revision,
            approval.content_digest,
            approval.workflow_id,
            approval.standard_base_digest,
            approval.reviewed_diff_digest,
        ),
        (
            current.expected_developer,
            current.room_id,
            current.draft_id,
            current.draft_revision,
            current.content_digest,
            current.workflow_id,
            current.standard_base_digest,
            current.reviewed_diff_digest,
        )
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn room_orchestrator_core_workflow_approval_rejects_symlinked_review_evidence() {
    use std::os::unix::fs::symlink;

    let root = temp_repo();
    let outside = temp_repo();
    let store = WorkflowDraftStore::new(root.clone()).unwrap();
    let mut ledger = WorkflowDraftLedger::default();
    let draft = store
        .persist(
            &mut ledger,
            RoomId(3),
            mutation(
                None,
                None,
                Some("review-sop"),
                "# Review SOP\n\n## Inspect\n\nUse confined facts.\n",
            ),
        )
        .unwrap();
    symlink(&outside, root.join(".bus/standard")).unwrap();
    assert!(matches!(
        store.promotion_review(&ledger, RoomId(3), &draft.draft_id),
        Err(WorkflowPromotionError::UnsafeTarget)
    ));
    assert_eq!(ledger.approvals().count(), 0);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(outside).unwrap();
}
