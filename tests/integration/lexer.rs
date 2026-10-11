//! Lexer integration tests: full-source tokenization, span precision, and
//! robustness against arbitrary input.

use proptest::prelude::*;
use vesper::lexer::tokenize;
use vesper::token::{Literal, TokenKind};

/// Kinds for a program exercising every category of token.
fn kinds(source: &str) -> Vec<TokenKind> {
    tokenize(source)
        .expect("source should lex without error")
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

#[test]
fn tokenizes_keywords_operators_and_literals() {
    let source = r#"
        and class else false for fun if nil or print return super this true var
        while break continue
        ( ) { } , . - + ; / * ! != = == < <= > >=
        "text" 42 3.5 1e3 identifier _private
    "#;
    let ks = kinds(source);
    for expected in [
        TokenKind::And,
        TokenKind::Class,
        TokenKind::Else,
        TokenKind::False,
        TokenKind::For,
        TokenKind::Fun,
        TokenKind::If,
        TokenKind::Nil,
        TokenKind::Or,
        TokenKind::Print,
        TokenKind::Return,
        TokenKind::Super,
        TokenKind::This,
        TokenKind::True,
        TokenKind::Var,
        TokenKind::While,
        TokenKind::Break,
        TokenKind::Continue,
        TokenKind::LeftParen,
        TokenKind::RightBrace,
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
        TokenKind::String,
        TokenKind::Number,
        TokenKind::Identifier,
        TokenKind::Eof,
    ] {
        assert!(ks.contains(&expected), "missing token kind: {expected:?}");
    }
}

#[test]
fn numbers_carry_their_literal_value() {
    let tokens = tokenize("1 2.5 1e3 2.5e-2").unwrap();
    let numbers: Vec<f64> = tokens
        .iter()
        .filter_map(|t| match t.literal {
            Some(Literal::Number(n)) => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(numbers, vec![1.0, 2.5, 1000.0, 0.025]);
}

#[test]
fn string_escapes_are_decoded() {
    let tokens = tokenize(r#""a\tb\nc\"d\\e""#).unwrap();
    let literal = tokens
        .iter()
        .find_map(|t| match &t.literal {
            Some(Literal::String(s)) => Some(s.clone()),
            _ => None,
        })
        .expect("one string literal");
    assert_eq!(literal, "a\tb\nc\"d\\e");
}

#[test]
fn comments_are_skipped() {
    let tokens = tokenize("// line comment\n/* block\n comment */ 7").unwrap();
    assert_eq!(tokens.len(), 2, "one number plus EOF");
    assert_eq!(tokens[0].kind, TokenKind::Number);
}

#[test]
fn error_reports_precise_span() {
    let errors = tokenize("var x = @;").unwrap_err();
    assert_eq!(errors.len(), 1);
    let span = errors[0].span().expect("lex error carries a span");
    assert_eq!(span.start, 8);
    assert_eq!(span.end, 9);
}

#[test]
fn unterminated_string_is_one_error() {
    let errors = tokenize("\"never ends").unwrap_err();
    assert_eq!(errors.len(), 1);
}

proptest! {
    /// The lexer must never panic, and must make progress, on arbitrary input.
    #[test]
    fn never_panics_on_arbitrary_input(source in "\\PC{0,64}") {
        let _ = tokenize(&source);
    }

    /// Every token stream produced by a successful lex ends in `EOF`.
    #[test]
    fn successful_stream_ends_with_eof(source in "\\PC{0,64}") {
        if let Ok(tokens) = tokenize(&source) {
            prop_assert_eq!(tokens.last().map(|t| t.kind), Some(TokenKind::Eof));
        }
    }
}
