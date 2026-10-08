use super::*;

#[test]
fn agent_wait_probe_only_translates_agent_disappearance() {
    let disappeared = agent_wait_probe_error(ErrorResponse {
        id: "wait".into(),
        error: ErrorBody {
            code: "agent_not_found".into(),
            message: "missing".into(),
        },
    })
    .unwrap();
    let disappeared: ErrorResponse = serde_json::from_str(&disappeared).unwrap();
    assert_eq!(disappeared.id, "wait");
    assert_eq!(disappeared.error.code, "agent_not_running");

    let unavailable = agent_wait_probe_error(ErrorResponse {
        id: "wait".into(),
        error: ErrorBody {
            code: "server_unavailable".into(),
            message: "timed out waiting for app response".into(),
        },
    })
    .unwrap();
    let unavailable: ErrorResponse = serde_json::from_str(&unavailable).unwrap();
    assert_eq!(unavailable.id, "wait");
    assert_eq!(unavailable.error.code, "server_unavailable");
}
