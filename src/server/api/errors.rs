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
            code: code.into(),
            message: message.into(),
        },
    )
}

pub(super) fn encode_error_body(id: String, error: ErrorBody) -> String {
    // ErrorResponse contains only strings. Serializing each as a JSON string
    // value is infallible, and this retains the schema's field order and escaping.
    format!(
        r#"{{"id":{},"error":{{"code":{},"message":{}}}}}"#,
        serde_json::Value::String(id),
        serde_json::Value::String(error.code),
        serde_json::Value::String(error.message),
    )
}
