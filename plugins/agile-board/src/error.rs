use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub i32, pub String);
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.1)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self(4, format!("local file I/O failed: {e}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self(4, format!("JSON serialization failed: {e}"))
    }
}

pub fn invalid(s: impl Into<String>) -> Error {
    Error(2, s.into())
}

pub fn transition_denied(s: impl Into<String>) -> Error {
    Error(3, s.into())
}

pub fn not_found(s: impl Into<String>) -> Error {
    Error(2, s.into())
}
