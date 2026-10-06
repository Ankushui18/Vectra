//! Logos lexer (Task 2.1 pipeline stage 1).
//!
//! `$`-prefixed idents are variables, bare idents are function names, and
//! unary minus is a parser concern (`Minus` + precedence), so `-1.5` and
//! `- $a` both work with no lexer special-casing.

use crate::error::ParseError;
use logos::Logos;
use std::fmt;
use std::ops::Range;

/// Expression tokens. Payloads are owned (`f64` / `String`) so the token
/// stream has no lifetime ties to the source text.
#[derive(Debug, Clone, PartialEq, Logos)]
#[logos(skip r"[ \t\n\r\f]+")]
pub enum Token {
    #[regex(r"(\d+\.\d*|\.\d+|\d+)([eE][+-]?\d+)?", |lex| lex.slice().parse::<f64>().unwrap_or(f64::NAN))]
    Number(f64),

    /// `$name` — payload is the name WITHOUT the sigil.
    #[regex(r"\$[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice()[1..].to_string())]
    Variable(String),

    /// Bare ident — only valid as a function name (checked in resolve).
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token(",")]
    Comma,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Variable(name) => write!(f, "${name}"),
            Self::Ident(name) => write!(f, "{name}"),
            Self::Plus => write!(f, "+"),
            Self::Minus => write!(f, "-"),
            Self::Star => write!(f, "*"),
            Self::Slash => write!(f, "/"),
            Self::LParen => write!(f, "("),
            Self::RParen => write!(f, ")"),
            Self::Comma => write!(f, ","),
        }
    }
}

/// Lex `src` into `(Token, byte-span)` pairs. Fails fast on the first invalid
/// character — fail-fast is correct for a single-line formula language (no
/// multi-error recovery needed).
pub fn lex(src: &str) -> Result<Vec<(Token, Range<usize>)>, ParseError> {
    if src.trim().is_empty() {
        return Err(ParseError::EmptyExpression);
    }
    let mut tokens = Vec::new();
    for (result, span) in Token::lexer(src).spanned() {
        match result {
            Ok(token) => tokens.push((token, span)),
            Err(()) => {
                return Err(ParseError::InvalidToken {
                    position: span.start,
                    text: src[span].to_string(),
                });
            }
        }
    }
    if tokens.is_empty() {
        return Err(ParseError::EmptyExpression);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Token> {
        lex(src).unwrap().into_iter().map(|(t, _)| t).collect()
    }

    #[test]
    fn tokenizes_mvp_example() {
        assert_eq!(
            kinds("$base * 2 + 10"),
            vec![
                Token::Variable("base".to_string()),
                Token::Star,
                Token::Number(2.0),
                Token::Plus,
                Token::Number(10.0),
            ]
        );
    }

    #[test]
    fn tokenizes_call_with_floats_and_comma() {
        assert_eq!(
            kinds("clamp($width, 0, 100.5)"),
            vec![
                Token::Ident("clamp".to_string()),
                Token::LParen,
                Token::Variable("width".to_string()),
                Token::Comma,
                Token::Number(0.0),
                Token::Comma,
                Token::Number(100.5),
                Token::RParen,
            ]
        );
    }

    #[test]
    fn number_forms() {
        assert_eq!(kinds("42"), vec![Token::Number(42.0)]);
        assert_eq!(kinds("2.5"), vec![Token::Number(2.5)]);
        assert_eq!(kinds(".5"), vec![Token::Number(0.5)]);
        assert_eq!(kinds("1e3"), vec![Token::Number(1000.0)]);
        assert_eq!(kinds("2.5e-2"), vec![Token::Number(0.025)]);
    }

    #[test]
    fn unary_minus_is_two_tokens() {
        assert_eq!(kinds("-1.5"), vec![Token::Minus, Token::Number(1.5)]);
    }

    #[test]
    fn invalid_characters_fail_with_position() {
        assert_eq!(
            lex("$a + \"x\""),
            Err(ParseError::InvalidToken {
                position: 5,
                text: "\"".to_string()
            })
        );
        assert_eq!(
            lex("1 @ 2"),
            Err(ParseError::InvalidToken {
                position: 2,
                text: "@".to_string()
            })
        );
    }

    #[test]
    fn blank_is_empty_expression() {
        assert_eq!(lex(""), Err(ParseError::EmptyExpression));
        assert_eq!(lex("   \n\t "), Err(ParseError::EmptyExpression));
    }
}
