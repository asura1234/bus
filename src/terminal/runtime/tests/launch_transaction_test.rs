use super::*;

#[test]
fn pending_verification_and_enter_in_flight_cannot_be_provider_exit() {
    for phase in [
        LaunchPhase::WaitingForPrompt,
        LaunchPhase::Verifying,
        LaunchPhase::Submitting,
    ] {
        let launch = LaunchTransaction::default();
        let id = launch.begin(0, 4);
        launch.transition(id, phase, 4);
        // Even an otherwise matching return cannot retire a writer whose
        // verified input transaction is still completing.
        assert!(!launch.returned(Some((5, id)), false), "{phase:?}");
    }
}

#[test]
fn return_requires_this_submitted_attempt_and_a_later_prompt_cycle() {
    let launch = LaunchTransaction::default();
    let id = launch.begin(0, 4);
    launch.transition(id, LaunchPhase::Submitting, 4);
    launch.transition(id, LaunchPhase::Submitted, 4);
    assert!(!launch.returned(Some((4, id)), false));
    assert!(!launch.returned(Some((5, id + 1)), false));
    assert!(!launch.returned(None, false));
    assert!(!launch.returned(Some((5, id)), true));
    assert!(launch.returned(Some((5, id)), false));
    assert!(launch.returned(Some((5, id)), false));
}

#[test]
fn stale_writer_and_old_return_cannot_mutate_a_replacement_attempt() {
    let launch = LaunchTransaction::default();
    let old = launch.begin(0, 4);
    launch.transition(old, LaunchPhase::Submitting, 4);
    let current = launch.begin(0, 4);
    assert!(!launch.transition(old, LaunchPhase::Submitted, 4));
    launch.fail(old, "late old write failure".into(), None);
    assert_eq!(launch.id(), Some(current));
    assert_eq!(launch.error(false), None);
    launch.transition(current, LaunchPhase::Submitted, 4);
    assert!(!launch.returned(Some((5, old)), false));
    assert!(launch.returned(Some((5, current)), false));
}

#[test]
fn uncertain_enter_needs_return_proof_but_pre_enter_failure_keeps_ownership() {
    for entered in [false, true] {
        let launch = LaunchTransaction::default();
        let id = launch.begin(0, 4);
        if entered {
            launch.transition(id, LaunchPhase::Submitting, 4);
        }
        launch.fail(id, "input failed".into(), None);
        assert_eq!(launch.error(false).as_deref(), Some("input failed"));
        assert_eq!(launch.error(true).is_none(), entered);
        assert!(!launch.returned(Some((4, id)), false));
        assert_eq!(launch.returned(Some((5, id)), false), entered);
    }
}
