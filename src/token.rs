//! Token definitions produced by the [`crate::lexer`].
//!
//! A [`Token`] couples a [`TokenKind`] (a cheap, `Copy` discriminant) with the
//! original lexeme, its [`Span`], and — for literals — the parsed
//! [`Literal`] payload. Keeping the lexeme allows the AST and error messages to
//! echo exactly what the user wrote.

use crate::span::Span;
use std::fmt;

/// A literal value decoded by the lexer.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    /// A floating-point number.
    Number(f64),
    /// A string literal, with escape sequences already processed.
    String(String),
    /// A boolean literal (`true` or `false`).
    Bool(bool),
    /// The `nil` literal.
    Nil,
}

/// The lexical class of a token.
///
/// # Design note
///
/// This is a closed set, so an enum is used rather than a trait. The variants
/// are ordered roughly by category: delimiters, operators, literals, keywords.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    // Single-character delimiters.
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Dot,
    Minus,
    Plus,
    Semicolon,
    Slash,
    Star,
    Colon,

    // One- or two-character operators.
    Bang,
    BangEqual,
    Equal,
    EqualEqual,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,

    // Literals.
    Identifier,
    String,
    Number,

    // Keywords.
    And,
    Break,
    Class,
    Continue,
    Else,
    False,
    For,
    Fun,
    If,
    Nil,
    Or,
    Print,
    Return,
    Super,
    This,
    True,
    Var,
    While,

    /// End of input.
    Eof,
}

impl TokenKind {
    /// Returns `true` if this token kind is a keyword or reserved word.
    #[must_use]
    pub const fn is_keyword(self) -> bool {
        matches!(
            self,
            Self::And
                | Self::Break
                | Self::Class
                | Self::Continue
                | Self::Else
                | Self::False
                | Self::For
                | Self::Fun
                | Self::If
                | Self::Nil
                | Self::Or
                | Self::Print
                | Self::Return
                | Self::Super
                | Self::This
                | Self::True
                | Self::Var
                | Self::While
        )
    }
}

/// A lexed token.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    /// The token's class.
    pub kind: TokenKind,
    /// The exact source text of the token.
    pub lexeme: String,
    /// The source span covering the token.
    pub span: Span,
    /// Parsed literal payload, if the token denotes a literal.
    pub literal: Option<Literal>,
}

impl Token {
    /// Constructs a token.
    #[must_use]
    pub fn new(kind: TokenKind, lexeme: String, span: Span, literal: Option<Literal>) -> Self {
        Self {
            kind,
            lexeme,
            span,
            literal,
        }
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} '{}'", self.kind, self.lexeme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_classification() {
        assert!(TokenKind::Fun.is_keyword());
        assert!(!TokenKind::Identifier.is_keyword());
    }

    #[test]
    fn token_construction() {
        let t = Token::new(
            TokenKind::Number,
            "42".to_string(),
            Span::new(0, 2),
            Some(Literal::Number(42.0)),
        );
        assert_eq!(t.kind, TokenKind::Number);
        assert_eq!(t.literal, Some(Literal::Number(42.0)));
    }
}
