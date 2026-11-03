//! Contract/corpus checks only. These do not run a lifecycle reducer or driver.
use super::{
    identity::{EndpointIdentity, NativeId, ObservationToken, SourceCursor},
    input::{Capabilities, InputAssessment, OperationScope, Readiness, Support, WriteReceipt},
    native::{
        ErrorEvidence, FactEnvelope, InputPart, NativeEvent, NativeReset, NativeTime, Outcome,
        ReadProgress,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const CORPUS: &str = include_str!("../../../../tests/fixtures/universal-driver/oracles.json");

#[derive(Debug, Serialize, Deserialize)]
struct Corpus {
    schema_version: u32,
    source_ref: String,
    reviewed_set: String,
    provenance: String,
    endpoint: EndpointIdentity,
    source: SourceCursor,
    capabilities: Capabilities,
    cases: Vec<ReplayCase>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReplayCase {
    id: String,
    title: String,
    synthetic_surface: String,
    native_events: Vec<NativeEvent>,
    write_receipts: Vec<WriteReceipt>,
    read_progress: Vec<ReadProgress>,
    expected: Oracle,
    baseline: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capabilities: Option<Capabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    endpoint: Option<EndpointIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    budget_oracle: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Oracle {
    public_state: String,
    readiness: Readiness,
    native_attempt: String,
    request: String,
    attention: Option<String>,
    uncertain_input_replay: bool,
}

fn corpus() -> Corpus {
    serde_json::from_str(CORPUS).expect("reviewed synthetic oracle catalogue")
}

#[test]
fn reviewed_catalogue_is_complete_and_preserves_its_wire_data() {
    let corpus = corpus();
    assert_eq!(corpus.schema_version, 1);
    assert_eq!(corpus.cases.len(), 25);
    assert!(corpus.provenance.starts_with("Synthetic normalized"));
    for (index, case) in corpus.cases.iter().enumerate() {
        assert_eq!(case.id, format!("A{:02}", index + 1));
        assert!(!case.title.is_empty());
        assert!(!case.expected.request.is_empty());
        assert!(!case.expected.uncertain_input_replay, "{}", case.id);
    }
    assert_eq!(
        serde_json::to_value(&corpus).unwrap(),
        serde_json::from_str::<Value>(CORPUS).unwrap(),
        "a contract change must not silently discard or rewrite fixture evidence"
    );
}

#[test]
fn native_envelopes_keep_namespace_identity_and_source_watermarks() {
    let corpus = corpus();
    let case = &corpus.cases[6];
    let mut envelopes = Vec::new();
    for event in &case.native_events {
        let envelope = FactEnvelope {
            endpoint: corpus.endpoint.clone(),
            source: corpus.source.clone(),
            observed_at_ms: 1000,
            native_time: Some(NativeTime {
                clock: "synthetic-wire".into(),
                millis: 900,
            }),
            raw: json!({"synthetic": true, "case": case.id}),
            event: event.clone(),
        };
        let encoded = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(
            serde_json::from_slice::<FactEnvelope>(&encoded).unwrap(),
            envelope
        );
        envelopes.push(envelope);
    }
    let NativeEvent::InputCommitObserved { attempt: outer, .. } = &envelopes[0].event else {
        panic!("outer commit fixture");
    };
    let NativeEvent::AttemptEndObserved { attempt: inner, .. } = &envelopes[1].event else {
        panic!("delayed inner end fixture");
    };
    assert_eq!(outer.value, inner.value);
    assert_ne!(outer, inner, "equal native numbers do not alias ID domains");
}

#[test]
fn native_errors_preserve_nested_json_and_native_reset_provenance() {
    let corpus = corpus();
    let NativeEvent::AttemptEndObserved {
        outcome: Outcome::Failed(error),
        ..
    } = &corpus.cases[2].native_events[0]
    else {
        panic!("native quota fixture");
    };
    assert!(error.raw["cause"]["error"]["detail"].is_array());
    assert_eq!(error.http_status, Some(429));
    assert!(error.native_reset.is_none(), "no invented reset deadline");
    let future = ErrorEvidence {
        raw: json!({"message": "original", "future": {"items": [null, 7, "文"]}}),
        message: None,
        code: Some("future.code".into()),
        http_status: None,
        native_reset: Some(NativeReset {
            at_ms: 21_601_000,
            source: corpus.source,
        }),
    };
    let decoded: ErrorEvidence =
        serde_json::from_value(serde_json::to_value(&future).unwrap()).unwrap();
    assert_eq!(decoded, future);
    assert!(
        decoded.raw.is_object(),
        "raw failure must not become [object Object]"
    );
}

#[test]
fn native_accepted_parts_keep_order_whitespace_and_attachment_identity() {
    let corpus = corpus();
    let NativeEvent::InputAccepted { attempt, parts, .. } = &corpus.cases[10].native_events[0]
    else {
        panic!("queued native input fixture");
    };
    assert!(
        attempt.is_none(),
        "acceptance need not have a started attempt"
    );
    assert_eq!(parts.first(), Some(&InputPart::Text("first\n".into())));
    assert!(
        matches!(&parts[1], InputPart::Attachment { native_id: Some(NativeId { namespace, .. }), .. } if namespace == "attachment")
    );
    assert_eq!(parts.last(), Some(&InputPart::Text("second  ".into())));
}

#[test]
fn transport_receipts_cannot_deserialize_as_native_terminal_evidence() {
    let corpus = corpus();
    for case in &corpus.cases {
        for receipt in &case.write_receipts {
            assert!(
                serde_json::from_value::<NativeEvent>(serde_json::to_value(receipt).unwrap())
                    .is_err()
            );
        }
    }
    assert_eq!(corpus.cases[19].expected.request, "stop_pending");
    assert_eq!(corpus.cases[19].expected.native_attempt, "open");
}

#[test]
fn incomplete_log_fragments_remain_lossless_and_distinct_from_gaps() {
    let corpus = corpus();
    let progress = &corpus.cases[24].read_progress;
    let ReadProgress::PendingTail {
        last_complete,
        fragment,
    } = &progress[0]
    else {
        panic!("growing tail fixture");
    };
    assert_eq!(last_complete, &corpus.source);
    assert!(std::str::from_utf8(fragment).is_err());
    assert!(
        matches!(&progress[1], ReadProgress::EvidenceGap { retained, .. } if retained == &[255, 0, 123])
    );
}

#[test]
fn declared_capabilities_are_independent_of_missing_record_evidence() {
    let corpus = corpus();
    assert!(corpus.cases[14].native_events.is_empty());
    assert_eq!(corpus.capabilities.native_lifecycle, Support::Present);
    let absent = corpus.cases[23].capabilities.as_ref().unwrap();
    assert_eq!(absent.native_lifecycle, Support::Absent);
    assert_eq!(absent.generic_lifecycle, Support::Absent);
    assert_eq!(
        corpus.cases[23].expected.attention.as_deref(),
        Some("lifecycle_unsupported")
    );
}

#[test]
fn operation_and_visual_assessment_share_identity_without_claiming_acceptance() {
    let corpus = corpus();
    let token = ObservationToken {
        endpoint: corpus.endpoint,
        content_revision: 12,
        native_through: Some(corpus.source),
    };
    let assessment = InputAssessment {
        token: token.clone(),
        readiness: Readiness::CandidateReady,
        evidence: vec!["synthetic owned input focus".into()],
    };
    let operation = OperationScope {
        id: "stop-operation".into(),
        kind: super::input::OperationKind::Stop,
        token,
        attempts: vec![NativeId {
            namespace: "outer-turn".into(),
            value: "1".into(),
        }],
        inputs: vec![],
    };
    let decoded: OperationScope =
        serde_json::from_value(serde_json::to_value(&operation).unwrap()).unwrap();
    assert_eq!(decoded.token, assessment.token);
    assert_eq!(decoded.attempts.len(), 1);
}
