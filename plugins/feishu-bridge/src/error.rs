use std::fmt;

#[derive(Debug)]
pub struct Error(pub i32, pub String);
pub type Result<T> = std::result::Result<T, Error>;
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.1)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self(4, "local file I/O failed".into())
    }
}
impl From<rusqlite::Error> for Error {
    fn from(_: rusqlite::Error) -> Self {
        Self(4, "SQLite state operation failed".into())
    }
}
pub fn invalid(s: &str) -> Error {
    Error(2, s.into())
}
pub fn upstream(s: &str) -> Error {
    Error(3, s.into())
}
