//! Splits a query into tokens, each with its span in characters.

use super::{QueryError, QueryErrorKind, Relative, Span, TimeUnit};

/// Largest amount of a relative duration (`-1000000d`): beyond, it means
/// nothing and dates overflow.
pub(super) const MAX_RELATIVE_AMOUNT: i64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TokenKind {
    LParen,
    RParen,
    Comma,
    /// `=`, `!=`, `<`, `<=`, `>`, `>=`, `~`, `!~`.
    Op(&'static str),
    /// A bare word: a field, a keyword, a function name or a value
    /// (`in_progress`, `PLAT-12`, `lucas@acme.example`, `2026-11-20`).
    Word(String),
    /// A quoted string, unescaped.
    Str(String),
    /// A number, without a leading `+`.
    Number(String),
    Relative(Relative),
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Whether `c` may be part of a bare word.
pub(super) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '@' | ':' | '/')
}

/// The tokens of `input`, ending with `End`.
pub(super) fn tokenize(input: &str) -> Result<Vec<Token>, QueryError> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let single = |kind| Token {
            kind,
            span: Span::new(i, 1),
        };
        let double = |kind| Token {
            kind,
            span: Span::new(i, 2),
        };
        let token = match c {
            _ if c.is_whitespace() => {
                i += 1;
                continue;
            }
            '(' => single(TokenKind::LParen),
            ')' => single(TokenKind::RParen),
            ',' => single(TokenKind::Comma),
            '=' => single(TokenKind::Op("=")),
            '~' => single(TokenKind::Op("~")),
            '!' if next == Some('=') => double(TokenKind::Op("!=")),
            '!' if next == Some('~') => double(TokenKind::Op("!~")),
            '<' if next == Some('=') => double(TokenKind::Op("<=")),
            '<' => single(TokenKind::Op("<")),
            '>' if next == Some('=') => double(TokenKind::Op(">=")),
            '>' => single(TokenKind::Op(">")),
            '"' | '\'' => string(&chars, i)?,
            _ if c.is_ascii_digit()
                || (matches!(c, '+' | '-') && next.is_some_and(|n| n.is_ascii_digit())) =>
            {
                numeric(&chars, i)?
            }
            _ if is_word_char(c) && c != '-' => {
                let end = run_end(&chars, i);
                Token {
                    kind: TokenKind::Word(chars[i..end].iter().collect()),
                    span: Span::new(i, end - i),
                }
            }
            _ => {
                return Err(QueryError::new(
                    Span::new(i, 1),
                    QueryErrorKind::InvalidCharacter(c),
                ));
            }
        };
        i = token.span.start + token.span.len;
        tokens.push(token);
    }
    tokens.push(Token {
        kind: TokenKind::End,
        span: Span::new(chars.len(), 0),
    });
    Ok(tokens)
}

/// The end of the run of word characters starting at `start`.
fn run_end(chars: &[char], start: usize) -> usize {
    let mut end = start + 1;
    while end < chars.len() && is_word_char(chars[end]) {
        end += 1;
    }
    end
}

/// A quoted string from `start` (its opening quote): `\` escapes the next
/// character.
fn string(chars: &[char], start: usize) -> Result<Token, QueryError> {
    let quote = chars[start];
    let mut text = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' if i + 1 < chars.len() => {
                text.push(chars[i + 1]);
                i += 2;
            }
            c if c == quote => {
                return Ok(Token {
                    kind: TokenKind::Str(text),
                    span: Span::new(start, i + 1 - start),
                });
            }
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
    Err(QueryError::new(
        Span::new(start, chars.len() - start),
        QueryErrorKind::UnterminatedString,
    ))
}

/// A number (`3`, `-2.5`), a duration (`-7d`, `2M`) or a word that starts
/// with a digit (`2026-11-20`).
fn numeric(chars: &[char], start: usize) -> Result<Token, QueryError> {
    let end = run_end(chars, start);
    let raw: String = chars[start..end].iter().collect();
    let span = Span::new(start, end - start);
    let signed = raw.starts_with(['+', '-']);
    let unsigned = raw.trim_start_matches(['+', '-']);
    let token = |kind| Ok(Token { kind, span });
    if is_number(unsigned) {
        let number = raw.strip_prefix('+').unwrap_or(&raw);
        return token(TokenKind::Number(number.to_owned()));
    }
    let mut digits = unsigned.chars();
    let unit = digits.next_back().and_then(TimeUnit::from_letter);
    let amount = digits.as_str();
    if let Some(unit) = unit
        && !amount.is_empty()
        && amount.chars().all(|c| c.is_ascii_digit())
    {
        let amount: i64 = amount
            .parse()
            .ok()
            .filter(|a| *a <= MAX_RELATIVE_AMOUNT)
            .ok_or_else(|| QueryError::new(span, QueryErrorKind::InvalidNumber(raw.clone())))?;
        let amount = if raw.starts_with('-') { -amount } else { amount };
        return token(TokenKind::Relative(Relative { amount, unit }));
    }
    if signed {
        return Err(QueryError::new(span, QueryErrorKind::InvalidNumber(raw)));
    }
    token(TokenKind::Word(raw))
}

/// Digits, optionally a dot and more digits.
pub(super) fn is_number(text: &str) -> bool {
    let (whole, fraction) = text.split_once('.').map_or((text, None), |(w, f)| (w, Some(f)));
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    digits(whole) && fraction.is_none_or(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(input: &str) -> Vec<TokenKind> {
        tokenize(input).unwrap().into_iter().map(|t| t.kind).collect()
    }

    fn word(w: &str) -> TokenKind {
        TokenKind::Word(w.to_owned())
    }

    #[test]
    fn splits_operators_words_strings_and_parentheses() {
        assert_eq!(
            kinds("status IN (todo,in_progress) AND summary~\"a \\\"b\\\"\" OR key!=PLAT-12"),
            [
                word("status"),
                word("IN"),
                TokenKind::LParen,
                word("todo"),
                TokenKind::Comma,
                word("in_progress"),
                TokenKind::RParen,
                word("AND"),
                word("summary"),
                TokenKind::Op("~"),
                TokenKind::Str("a \"b\"".to_owned()),
                word("OR"),
                word("key"),
                TokenKind::Op("!="),
                word("PLAT-12"),
                TokenKind::End,
            ]
        );
        for (op, kind) in [
            ("<=", "<="),
            (">=", ">="),
            ("<", "<"),
            (">", ">"),
            ("!~", "!~"),
            ("=", "="),
        ] {
            assert_eq!(kinds(&format!("a{op}b"))[1], TokenKind::Op(kind));
        }
        assert_eq!(
            kinds("'it''s'")[0],
            TokenKind::Str("it".to_owned()),
            "single quotes too"
        );
        assert_eq!(
            kinds("assignee = lucas@acme.example")[2],
            word("lucas@acme.example")
        );
    }

    #[test]
    fn reads_numbers_durations_and_dates() {
        assert_eq!(kinds("3")[0], TokenKind::Number("3".to_owned()));
        assert_eq!(kinds("-2.5")[0], TokenKind::Number("-2.5".to_owned()));
        assert_eq!(kinds("+4")[0], TokenKind::Number("4".to_owned()));
        assert_eq!(
            kinds("-7d")[0],
            TokenKind::Relative(Relative {
                amount: -7,
                unit: TimeUnit::Days
            })
        );
        assert_eq!(
            kinds("2M")[0],
            TokenKind::Relative(Relative {
                amount: 2,
                unit: TimeUnit::Months
            })
        );
        assert_eq!(
            kinds("+30m")[0],
            TokenKind::Relative(Relative {
                amount: 30,
                unit: TimeUnit::Minutes
            })
        );
        assert_eq!(kinds("2026-11-20")[0], word("2026-11-20"));
        assert_eq!(kinds("12x")[0], word("12x"));
    }

    #[test]
    fn spans_count_characters_not_bytes() {
        let tokens = tokenize("résumé ~ \"été\" x").unwrap();
        let spans: Vec<Span> = tokens.iter().map(|t| t.span).collect();
        assert_eq!(
            spans,
            [
                Span::new(0, 6),
                Span::new(7, 1),
                Span::new(9, 5),
                Span::new(15, 1),
                Span::new(16, 0)
            ]
        );
    }

    #[test]
    fn refuses_what_it_cannot_read_with_its_place() {
        let error = |input: &str| tokenize(input).unwrap_err();
        assert_eq!(
            error("summary ~ \"open"),
            QueryError::new(Span::new(10, 5), QueryErrorKind::UnterminatedString)
        );
        assert_eq!(
            error("a = b; DROP"),
            QueryError::new(Span::new(5, 1), QueryErrorKind::InvalidCharacter(';'))
        );
        assert_eq!(
            error("a ! b"),
            QueryError::new(Span::new(2, 1), QueryErrorKind::InvalidCharacter('!'))
        );
        assert_eq!(
            error("due < -7x").kind,
            QueryErrorKind::InvalidNumber("-7x".to_owned())
        );
        assert_eq!(
            error("due < -9999999d").kind,
            QueryErrorKind::InvalidNumber("-9999999d".to_owned())
        );
    }
}
