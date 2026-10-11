//! The lexer: converts UTF-8 source text into a stream of [`Token`]s.
//!
//! # Design
//!
//! The lexer is a single-pass, hand-written scanner over the byte representation
//! of the source. Bytes are appropriate here because all structural characters
//! (delimiters, operators, ASCII keywords) are single-byte in UTF-8; multi-byte
//! sequences only ever occur inside string literals, where they are copied
//! through verbatim. UTF-8 is self-synchronizing, so scanning for ASCII
//! sentinels never splits a character.
//!
//! # Invariants
//!
//! * `current` never exceeds `source.len()`.
//! * Every call to [`Lexer::next_token`] either returns a token or returns an
//!   error **after advancing** at least one byte, guaranteeing termination of
//!   [`Lexer::scan_tokens`].
//! * `start <= current` at all times, so every produced span is well-formed.

use crate::error::Error;
use crate::span::Span;
use crate::token::{Literal, Token, TokenKind};

/// Hand-written scanner over a source string.
#[derive(Debug)]
pub struct Lexer<'src> {
    source: &'src str,
    bytes: &'src [u8],
    start: usize,
    current: usize,
}

impl<'src> Lexer<'src> {
    /// Creates a lexer over `source`.
    #[must_use]
    pub fn new(source: &'src str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            start: 0,
            current: 0,
        }
    }

    /// The source this lexer scans.
    #[must_use]
    pub const fn source(&self) -> &'src str {
        self.source
    }

    /// Scans all tokens, collecting recoverable errors instead of stopping at
    /// the first one. The returned token vector always ends with
    /// [`TokenKind::Eof`].
    #[must_use]
    pub fn scan_tokens(&mut self) -> (Vec<Token>, Vec<Error>) {
        let mut tokens = Vec::new();
        let mut errors = Vec::new();
        loop {
            let before = self.current;
            match self.next_token() {
                Ok(tok) => {
                    let eof = tok.kind == TokenKind::Eof;
                    tokens.push(tok);
                    if eof {
                        break;
                    }
                }
                Err(err) => {
                    errors.push(err);
                    // Safety net: guarantee forward progress even if an error
                    // path forgot to advance, so the loop cannot spin forever.
                    if self.current == before && !self.is_at_end() {
                        self.current += 1;
                    }
                }
            }
        }
        (tokens, errors)
    }

    /// Produces the next token, or a [`Error::Lexer`] describing the problem.
    ///
    /// Contract (post): the lexer's cursor has advanced past the returned
    /// token's lexeme, or to end-of-input, on both success and error.
    pub fn next_token(&mut self) -> Result<Token, Error> {
        self.skip_trivia()?;
        self.start = self.current;

        if self.is_at_end() {
            return Ok(Token::new(TokenKind::Eof, String::new(), self.span(), None));
        }

        let c = self.advance();
        let tok = match c {
            b'(' => self.simple(TokenKind::LeftParen),
            b')' => self.simple(TokenKind::RightParen),
            b'{' => self.simple(TokenKind::LeftBrace),
            b'}' => self.simple(TokenKind::RightBrace),
            b'[' => self.simple(TokenKind::LeftBracket),
            b']' => self.simple(TokenKind::RightBracket),
            b',' => self.simple(TokenKind::Comma),
            b'.' => self.simple(TokenKind::Dot),
            b'-' => self.simple(TokenKind::Minus),
            b'+' => self.simple(TokenKind::Plus),
            b';' => self.simple(TokenKind::Semicolon),
            b':' => self.simple(TokenKind::Colon),
            b'*' => self.simple(TokenKind::Star),
            b'!' => {
                let kind = if self.matches(b'=') {
                    TokenKind::BangEqual
                } else {
                    TokenKind::Bang
                };
                self.simple(kind)
            }
            b'=' => {
                let kind = if self.matches(b'=') {
                    TokenKind::EqualEqual
                } else {
                    TokenKind::Equal
                };
                self.simple(kind)
            }
            b'<' => {
                let kind = if self.matches(b'=') {
                    TokenKind::LessEqual
                } else {
                    TokenKind::Less
                };
                self.simple(kind)
            }
            b'>' => {
                let kind = if self.matches(b'=') {
                    TokenKind::GreaterEqual
                } else {
                    TokenKind::Greater
                };
                self.simple(kind)
            }
            b'/' => self.simple(TokenKind::Slash),
            b'"' => self.string()?,
            b'0'..=b'9' => self.number(),
            _ if is_ident_start(c) => self.identifier(),
            _ => {
                // Consume the remainder of a multi-byte UTF-8 character so the
                // next token starts on a character boundary and one invalid
                // character produces exactly one diagnostic.
                while self.current < self.bytes.len() && (self.bytes[self.current] & 0xC0) == 0x80 {
                    self.current += 1;
                }
                let span = self.span();
                return Err(Error::lexer(
                    format!(
                        "unexpected character '{}'",
                        char_from_byte(self.source, self.start)
                    ),
                    span,
                ));
            }
        };
        Ok(tok)
    }

    // --- Trivia -----------------------------------------------------------

    /// Skips whitespace and comments. Handles nested `/* ... */` block
    /// comments, which is a deliberate (documented) extension for ergonomics.
    fn skip_trivia(&mut self) -> Result<(), Error> {
        loop {
            let c = self.peek();
            match c {
                b' ' | b'\r' | b'\t' => {
                    self.advance();
                }
                b'\n' => {
                    self.advance();
                }
                b'/' if self.peek_next() == b'/' => {
                    while self.peek() != b'\n' && !self.is_at_end() {
                        self.advance();
                    }
                }
                b'/' if self.peek_next() == b'*' => {
                    self.advance();
                    self.advance();
                    let open_span = Span::new(self.start, self.current);
                    let mut depth = 1usize;
                    while depth > 0 {
                        if self.is_at_end() {
                            return Err(Error::lexer("unterminated block comment", open_span));
                        }
                        if self.peek() == b'/' && self.peek_next() == b'*' {
                            self.advance();
                            self.advance();
                            depth += 1;
                        } else if self.peek() == b'*' && self.peek_next() == b'/' {
                            self.advance();
                            self.advance();
                            depth -= 1;
                        } else {
                            self.advance();
                        }
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    // --- Token constructors ----------------------------------------------

    fn simple(&self, kind: TokenKind) -> Token {
        self.token(kind, None)
    }

    fn token(&self, kind: TokenKind, literal: Option<Literal>) -> Token {
        Token::new(
            kind,
            self.source[self.start..self.current].to_string(),
            self.span(),
            literal,
        )
    }

    fn string(&mut self) -> Result<Token, Error> {
        let mut buf: Vec<u8> = Vec::new();
        while !self.is_at_end() && self.peek() != b'"' {
            let c = self.advance();
            if c == b'\\' {
                if self.is_at_end() {
                    break;
                }
                let esc = self.advance();
                match esc {
                    b'n' => buf.push(b'\n'),
                    b't' => buf.push(b'\t'),
                    b'r' => buf.push(b'\r'),
                    b'\\' => buf.push(b'\\'),
                    b'"' => buf.push(b'"'),
                    b'0' => buf.push(0),
                    _ => {
                        let span = Span::new(self.current.saturating_sub(2), self.current);
                        return Err(Error::lexer(
                            format!(
                                "invalid escape sequence '\\{}'",
                                char_from_byte(self.source, span.start)
                            ),
                            span,
                        ));
                    }
                }
            } else {
                buf.push(c);
            }
        }

        if self.is_at_end() {
            return Err(Error::lexer("unterminated string", self.span()));
        }
        // Consume the closing quote.
        self.advance();

        // `from_utf8` cannot fail: valid UTF-8 bytes copied verbatim and ASCII
        // escapes preserve well-formedness.
        let value = String::from_utf8(buf).unwrap_or_default();
        Ok(self.token(TokenKind::String, Some(Literal::String(value))))
    }

    fn number(&mut self) -> Token {
        while is_digit(self.peek()) {
            self.advance();
        }
        // Fractional part.
        if self.peek() == b'.' && is_digit(self.peek_next()) {
            self.advance();
            while is_digit(self.peek()) {
                self.advance();
            }
        }
        // Optional scientific notation.
        if matches!(self.peek(), b'e' | b'E') {
            let next = self.peek_next();
            let after = self.peek_at(2);
            let signed_digit = (next == b'+' || next == b'-') && is_digit(after);
            if is_digit(next) || signed_digit {
                self.advance(); // e/E
                if matches!(self.peek(), b'+' | b'-') {
                    self.advance();
                }
                while is_digit(self.peek()) {
                    self.advance();
                }
            }
        }
        let text = &self.source[self.start..self.current];
        let value = text.parse::<f64>().unwrap_or(f64::NAN);
        self.token(TokenKind::Number, Some(Literal::Number(value)))
    }

    fn identifier(&mut self) -> Token {
        while is_ident_continue(self.peek()) {
            self.advance();
        }
        let text = &self.source[self.start..self.current];
        let kind = keyword(text).unwrap_or(TokenKind::Identifier);
        match kind {
            TokenKind::True => self.token(kind, Some(Literal::Bool(true))),
            TokenKind::False => self.token(kind, Some(Literal::Bool(false))),
            TokenKind::Nil => self.token(kind, Some(Literal::Nil)),
            _ => self.token(kind, None),
        }
    }

    // --- Cursor primitives ------------------------------------------------

    #[must_use]
    fn is_at_end(&self) -> bool {
        self.current >= self.bytes.len()
    }

    fn advance(&mut self) -> u8 {
        let c = self.bytes.get(self.current).copied().unwrap_or(0);
        self.current += 1;
        c
    }

    fn matches(&mut self, expected: u8) -> bool {
        if self.peek() == expected {
            self.current += 1;
            true
        } else {
            false
        }
    }

    #[must_use]
    fn peek(&self) -> u8 {
        self.bytes.get(self.current).copied().unwrap_or(0)
    }

    #[must_use]
    fn peek_next(&self) -> u8 {
        self.bytes.get(self.current + 1).copied().unwrap_or(0)
    }

    #[must_use]
    fn peek_at(&self, offset: usize) -> u8 {
        self.bytes.get(self.current + offset).copied().unwrap_or(0)
    }

    #[must_use]
    const fn span(&self) -> Span {
        Span::new(self.start, self.current)
    }
}

/// Lexes `source`, returning all tokens on success or every recoverable error
/// on failure. On failure the token stream is unavailable, which keeps callers
/// from attempting to parse a partial program.
pub fn tokenize(source: &str) -> Result<Vec<Token>, Vec<Error>> {
    let mut lexer = Lexer::new(source);
    let (tokens, errors) = lexer.scan_tokens();
    if errors.is_empty() {
        Ok(tokens)
    } else {
        Err(errors)
    }
}

/// Decodes the character beginning at byte `at` (used only for diagnostics).
///
/// Returns [`char::REPLACEMENT_CHARACTER`] if `at` is out of range or not on a
/// UTF-8 character boundary, so diagnostics can never panic on malformed input.
fn char_from_byte(source: &str, at: usize) -> char {
    source
        .get(at..)
        .and_then(|s| s.chars().next())
        .unwrap_or(char::REPLACEMENT_CHARACTER)
}

#[must_use]
const fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

#[must_use]
const fn is_ident_start(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphabetic()
}

#[must_use]
const fn is_ident_continue(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

/// Maps a lexeme to its keyword [`TokenKind`], if any.
#[must_use]
fn keyword(text: &str) -> Option<TokenKind> {
    Some(match text {
        "and" => TokenKind::And,
        "break" => TokenKind::Break,
        "class" => TokenKind::Class,
        "continue" => TokenKind::Continue,
        "else" => TokenKind::Else,
        "false" => TokenKind::False,
        "for" => TokenKind::For,
        "fun" => TokenKind::Fun,
        "if" => TokenKind::If,
        "nil" => TokenKind::Nil,
        "or" => TokenKind::Or,
        "print" => TokenKind::Print,
        "return" => TokenKind::Return,
        "super" => TokenKind::Super,
        "this" => TokenKind::This,
        "true" => TokenKind::True,
        "var" => TokenKind::Var,
        "while" => TokenKind::While,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        tokenize(src)
            .expect("expected successful tokenization")
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn empty_input_yields_eof() {
        assert_eq!(kinds(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn punctuators_and_operators() {
        assert_eq!(
            kinds("(){}[],.-+; / * ! != = == < <= > >="),
            vec![
                TokenKind::LeftParen,
                TokenKind::RightParen,
                TokenKind::LeftBrace,
                TokenKind::RightBrace,
                TokenKind::LeftBracket,
                TokenKind::RightBracket,
                TokenKind::Comma,
                TokenKind::Dot,
                TokenKind::Minus,
                TokenKind::Plus,
                TokenKind::Semicolon,
                TokenKind::Slash,
                TokenKind::Star,
                TokenKind::Bang,
                TokenKind::BangEqual,
                TokenKind::Equal,
                TokenKind::EqualEqual,
                TokenKind::Less,
                TokenKind::LessEqual,
                TokenKind::Greater,
                TokenKind::GreaterEqual,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keywords_are_recognized() {
        let src = "and class else false for fun if nil or print return super this true var while break continue";
        let ks = kinds(src);
        assert!(ks.contains(&TokenKind::And));
        assert!(ks.contains(&TokenKind::Class));
        assert!(ks.contains(&TokenKind::While));
        assert!(ks.contains(&TokenKind::Break));
        assert!(ks.contains(&TokenKind::Eof));
    }

    #[test]
    fn numbers_with_fraction_and_exponent() {
        let toks = tokenize("1 2.5 3.25 1e3 2.5e-2").unwrap();
        let nums: Vec<f64> = toks
            .iter()
            .filter_map(|t| match t.literal {
                Some(Literal::Number(n)) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(nums, vec![1.0, 2.5, 3.25, 1000.0, 0.025]);
    }

    #[test]
    fn string_literals_with_escapes() {
        let toks = tokenize(r#""hello\n\t\"world\"""#).unwrap();
        assert_eq!(toks[0].kind, TokenKind::String);
        assert_eq!(
            toks[0].literal,
            Some(Literal::String("hello\n\t\"world\"".to_string()))
        );
    }

    #[test]
    fn identifier_boundaries() {
        let toks = tokenize("foo_bar baz123 _x9").unwrap();
        assert_eq!(toks[0].lexeme, "foo_bar");
        assert_eq!(toks[1].lexeme, "baz123");
        assert_eq!(toks[2].lexeme, "_x9");
    }

    #[test]
    fn comments_are_skipped() {
        let toks = tokenize("1 // line comment\n /* block /* nested */ */ 2").unwrap();
        let nums: Vec<f64> = toks
            .iter()
            .filter_map(|t| match t.literal {
                Some(Literal::Number(n)) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(nums, vec![1.0, 2.0]);
    }

    #[test]
    fn unterminated_string_is_error() {
        let err = tokenize("\"oops").unwrap_err();
        assert_eq!(err[0].category(), "lex");
    }

    #[test]
    fn unterminated_block_comment_is_error() {
        let err = tokenize("/* nope").unwrap_err();
        assert_eq!(err[0].category(), "lex");
    }

    #[test]
    fn unexpected_character_is_error() {
        let err = tokenize("let $ = 1;").unwrap_err();
        assert_eq!(err[0].category(), "lex");
    }

    #[test]
    fn invalid_escape_is_error() {
        let err = tokenize(r#""bad\q""#).unwrap_err();
        assert_eq!(err[0].category(), "lex");
    }

    #[test]
    fn multiline_errors_recover_and_report() {
        // Two errors: `$` and `#`.
        let (_tokens, errors) = Lexer::new("$ #").scan_tokens();
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn spans_are_correct() {
        let toks = tokenize("var x = 42;").unwrap();
        assert_eq!(toks[0].span, Span::new(0, 3));
        assert_eq!(toks[1].span, Span::new(4, 5));
        assert_eq!(toks[2].span, Span::new(6, 7));
        assert_eq!(toks[3].span, Span::new(8, 10));
        assert_eq!(toks[4].span, Span::new(10, 11));
    }

    proptest! {
        /// Property: lexing arbitrary input never panics and always consumes
        /// the entire input (terminating scan).
        #[test]
        fn never_panics_and_terminates(s in ".*") {
            let mut lexer = Lexer::new(&s);
            let (_tokens, _errors) = lexer.scan_tokens();
            // Reaching here without panic is the assertion.
        }

        /// Property: a successful token stream always ends with EOF.
        #[test]
        fn successful_stream_ends_with_eof(s in "[a-zA-Z0-9_ +*/().;]{0,40}") {
            if let Ok(tokens) = tokenize(&s) {
                prop_assert_eq!(tokens.last().map(|t| t.kind), Some(TokenKind::Eof));
            }
        }
    }
}
