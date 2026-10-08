use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) enum AttachmentError {
    NotAbsolute(PathBuf),
    NotFound {
        path: PathBuf,
        source: std::io::Error,
    },
    NotFile(PathBuf),
    Canonicalize {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAbsolute(path) => {
                write!(formatter, "Use an absolute file path: {}", path.display())
            }
            Self::NotFound { path, source } => {
                write!(formatter, "Cannot read {}: {source}", path.display())
            }
            Self::NotFile(path) => write!(formatter, "Not a regular file: {}", path.display()),
            Self::Canonicalize { path, source } => {
                write!(formatter, "Cannot resolve {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for AttachmentError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PathParseError {
    UnterminatedQuote,
    TrailingEscape,
}

impl std::fmt::Display for PathParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "invalid pasted path list: {self:?}")
    }
}

impl std::error::Error for PathParseError {}

pub(crate) fn validate_attachment(input: &str, home: &Path) -> Result<PathBuf, AttachmentError> {
    let path = if input == "~" {
        home.to_path_buf()
    } else if let Some(remainder) = input.strip_prefix("~/") {
        home.join(remainder)
    } else {
        PathBuf::from(input)
    };
    if !path.is_absolute() {
        return Err(AttachmentError::NotAbsolute(path));
    }
    let metadata = std::fs::metadata(&path).map_err(|source| AttachmentError::NotFound {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(AttachmentError::NotFile(path));
    }
    path.canonicalize()
        .map_err(|source| AttachmentError::Canonicalize { path, source })
}

pub(crate) fn parse_path_tokens(input: &str) -> Result<Vec<String>, PathParseError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = Quote::None;
    let mut token_started = false;
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        match quote {
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::None;
                } else {
                    token.push(character);
                }
            }
            Quote::Double => match character {
                '"' => quote = Quote::None,
                '\\' => match characters.peek().copied() {
                    Some('"' | '\\') => {
                        token.push(characters.next().unwrap_or_default());
                    }
                    _ => token.push('\\'),
                },
                _ => token.push(character),
            },
            Quote::None => match character {
                '\'' => {
                    quote = Quote::Single;
                    token_started = true;
                }
                '"' => {
                    quote = Quote::Double;
                    token_started = true;
                }
                '\\' => {
                    token_started = true;
                    let Some(escaped) = characters.next() else {
                        return Err(PathParseError::TrailingEscape);
                    };
                    token.push(escaped);
                }
                character if character.is_whitespace() => {
                    if token_started {
                        tokens.push(std::mem::take(&mut token));
                        token_started = false;
                    }
                }
                _ => {
                    token.push(character);
                    token_started = true;
                }
            },
        }
    }
    if quote != Quote::None {
        return Err(PathParseError::UnterminatedQuote);
    }
    if token_started {
        tokens.push(token);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bus-files-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn attachment_expands_home_requires_absolute_regular_file_and_preserves_full_path() {
        let home = temp_dir("home");
        let file = home.join("a report @two.md");
        fs::write(&file, b"test").expect("write file");
        let directory = home.join("folder");
        fs::create_dir(&directory).expect("directory");

        assert_eq!(
            validate_attachment("~/a report @two.md", &home).expect("valid"),
            file.canonicalize().expect("canonical file")
        );
        assert!(matches!(
            validate_attachment("relative.md", &home),
            Err(AttachmentError::NotAbsolute(_))
        ));
        assert!(matches!(
            validate_attachment(directory.to_str().expect("utf8"), &home),
            Err(AttachmentError::NotFile(_))
        ));
        fs::remove_dir_all(home).expect("cleanup");
    }

    #[test]
    fn quoted_path_parser_handles_spaces_without_executing_shell_syntax() {
        assert_eq!(
            parse_path_tokens("'/tmp/one file.md' \"/tmp/two file.md\" /tmp/plain.md")
                .expect("tokens"),
            vec!["/tmp/one file.md", "/tmp/two file.md", "/tmp/plain.md"]
        );
        assert_eq!(
            parse_path_tokens("'/tmp/$(touch SHOULD_NOT_EXIST).md'").expect("literal"),
            vec!["/tmp/$(touch SHOULD_NOT_EXIST).md"]
        );
        assert_eq!(
            parse_path_tokens(r#""C:\Users\Dylan Liu\file.md""#).expect("windows path"),
            vec![r#"C:\Users\Dylan Liu\file.md"#]
        );
        assert!(matches!(
            parse_path_tokens("'/tmp/unterminated"),
            Err(PathParseError::UnterminatedQuote)
        ));
    }
}
