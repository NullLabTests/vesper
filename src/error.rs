//! Unified, typed error handling for every compiler stage.
//!
//! All fallible stages return [`Result<T, Error>`](Error). Errors are never
//! swallowed; every `?` propagates a typed variant that carries a [`Span`]
//! (except [`Error::Io`], which originates outside the language pipeline).
//!
//! Use [`Error::render`] to produce a human-readable, multi-line diagnostic
//! with the offending source line and a caret.

use crate::span::Span;
use std::fmt::Write as _;
use thiserror::Error;

/// The single error type flowing through the lexer, parser, resolver,
/// compiler, and VM.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// A lexing (tokenization) error.
    #[error("lex error: {message}")]
    Lexer {
        /// Human-readable description of the problem.
        message: String,
        /// Location of the offending text.
        span: Span,
    },
    /// A parsing (syntax) error.
    #[error("parse error: {message}")]
    Parse {
        /// Human-readable description of the problem.
        message: String,
        /// Location of the offending token.
        span: Span,
    },
    /// A compile-time (static) error, including resolution failures.
    #[error("compile error: {message}")]
    Compile {
        /// Human-readable description of the problem.
        message: String,
        /// Location of the offending AST node.
        span: Span,
    },
    /// A runtime (dynamic) error raised by the VM or tree-walker.
    #[error("runtime error: {message}")]
    Runtime {
        /// Human-readable description of the problem.
        message: String,
        /// Location of the currently executing bytecode.
        span: Span,
    },
    /// An I/O error, e.g. reading a source file.
    #[error("io error: {0}")]
    Io(String),
}

impl Error {
    /// Convenience constructor for a lexer error.
    pub fn lexer(message: impl Into<String>, span: Span) -> Self {
        Self::Lexer {
            message: message.into(),
            span,
        }
    }

    /// Convenience constructor for a parse error.
    pub fn parse(message: impl Into<String>, span: Span) -> Self {
        Self::Parse {
            message: message.into(),
            span,
        }
    }

    /// Convenience constructor for a compile error.
    pub fn compile(message: impl Into<String>, span: Span) -> Self {
        Self::Compile {
            message: message.into(),
            span,
        }
    }

    /// Convenience constructor for a runtime error.
    pub fn runtime(message: impl Into<String>, span: Span) -> Self {
        Self::Runtime {
            message: message.into(),
            span,
        }
    }

    /// Returns the source span associated with the error, if any.
    #[must_use]
    pub const fn span(&self) -> Option<Span> {
        match self {
            Self::Lexer { span, .. }
            | Self::Parse { span, .. }
            | Self::Compile { span, .. }
            | Self::Runtime { span, .. } => Some(*span),
            Self::Io(_) => None,
        }
    }

    /// A short machine-friendly category name (`lex`, `parse`, `compile`,
    /// `runtime`, `io`).
    #[must_use]
    pub const fn category(&self) -> &'static str {
        match self {
            Self::Lexer { .. } => "lex",
            Self::Parse { .. } => "parse",
            Self::Compile { .. } => "compile",
            Self::Runtime { .. } => "runtime",
            Self::Io(_) => "io",
        }
    }

    /// Renders the error with surrounding source context: a `file:line:col`
    /// header, the offending line, and a caret underline.
    ///
    /// `source_name` is a display name such as a file path or `<repl>`.
    #[must_use]
    pub fn render(&self, source_name: &str, source: &str) -> String {
        let Some(span) = self.span() else {
            return format!("error: {self}");
        };

        let (line, col) = span.line_col(source);
        let mut out = String::new();
        let _ = writeln!(out, "{source_name}:{line}:{col}: {self}");

        // Extract the source line containing the start offset.
        if let Some(line_text) = nth_line(source, line) {
            let _ = writeln!(out, "  |");
            let _ = writeln!(out, "{line:>3} | {line_text}");
            // Build the caret underline. Width is the span length clamped to
            // the remainder of the line, and at least one caret.
            let width = caret_width(span, line_text, col);
            let _ = writeln!(
                out,
                "  | {}{}",
                " ".repeat(col.saturating_sub(1)),
                "^".repeat(width)
            );
        }
        out
    }
}

/// Returns the 1-based `n`th line of `source`, without the trailing newline.
fn nth_line(source: &str, n: usize) -> Option<&str> {
    source.lines().nth(n.saturating_sub(1))
}

/// Computes a sensible caret width for a span on a single source line.
fn caret_width(span: Span, line_text: &str, col: usize) -> usize {
    let remaining = line_text
        .chars()
        .count()
        .saturating_sub(col.saturating_sub(1));
    let span_chars = span.len().max(1);
    span_chars.min(remaining.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_names() {
        assert_eq!(Error::lexer("x", Span::new(0, 1)).category(), "lex");
        assert_eq!(Error::parse("x", Span::new(0, 1)).category(), "parse");
        assert_eq!(Error::compile("x", Span::new(0, 1)).category(), "compile");
        assert_eq!(Error::runtime("x", Span::new(0, 1)).category(), "runtime");
        assert_eq!(Error::Io("x".into()).category(), "io");
    }

    #[test]
    fn io_has_no_span() {
        assert!(Error::Io("boom".into()).span().is_none());
    }

    #[test]
    fn render_includes_caret() {
        let err = Error::parse("unexpected token", Span::new(0, 3));
        let rendered = err.render("<test>", "abc def");
        assert!(rendered.contains("<test>:1:1"));
        assert!(rendered.contains('^'));
        assert!(rendered.contains("abc def"));
    }

    #[test]
    fn render_handles_io_without_span() {
        let rendered = Error::Io("disk".into()).render("<x>", "");
        assert_eq!(rendered, "error: io error: disk");
    }
}
