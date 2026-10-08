//! Literal shell-style tokenization shared by pasted paths and launch argv.

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
