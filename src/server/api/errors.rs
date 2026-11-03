use crate::protocol::api::schema::{ErrorBody, ResponseResult, SuccessResponse};

pub(crate) fn encode_success(id: String, result: ResponseResult) -> String {
    match serde_json::to_string(&SuccessResponse {
        id: id.clone(),
        result,
    }) {
        Ok(response) => response,
        Err(error) => encode_error(id, "serialization_error", error.to_string()),
    }
}

pub(crate) fn encode_error(id: String, code: &str, message: impl Into<String>) -> String {
    encode_error_body(
        id,
        ErrorBody {
            timestamps: None,
            code: code.into(),
            message: message.into(),
        },
    )
}

pub(super) fn encode_error_body(id: String, error: ErrorBody) -> String {
    let timestamps = error.timestamps.map(|value| {
        format!(
            r#", "timestamps":{}"#,
            serde_json::to_string(&value).unwrap_or_else(|_| "null".into())
        )
    });
    format!(
        r#"{{"id":{},"error":{{"code":{},"message":{}{}}}}}"#,
        serde_json::Value::String(id),
        serde_json::Value::String(error.code),
        serde_json::Value::String(error.message),
        timestamps.unwrap_or_default(),
    )
}
