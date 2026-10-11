//! Source location tracking.
//!
//! Every token, AST node, and diagnostic carries a [`Span`], a half-open byte
//! range `[start, end)` into the original UTF-8 source text. Byte offsets (as
//! opposed to `(line, column)` pairs) are cheap to copy, easy to merge, and
//! convertible to human-readable positions on demand via [`Span::line_col`].
//!
//! # Invariants
//!
//! * `start <= end` for every non-`dummy` span produced by the lexer.
//! * Offsets always fall on UTF-8 character boundaries because the lexer only
//!   advances at character boundaries.

use std::fmt;

/// A half-open byte range `[start, end)` into a source file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl Span {
    /// Creates a new span.
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Returns a placeholder span, used when no meaningful location exists
    /// (for example, compiler-synthesized code such as `for` desugaring).
    #[must_use]
    pub const fn dummy() -> Self {
        Self { start: 0, end: 0 }
    }

    /// Returns the smallest span covering both `self` and `other`.
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Returns the length in bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Returns `true` if the span is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Computes the 1-based `(line, column)` of the start offset.
    ///
    /// Column counts Unicode scalar values (characters), not bytes, so the
    /// caret in an error message lines up with a monospace terminal.
    #[must_use]
    pub fn line_col(&self, source: &str) -> (usize, usize) {
        let mut line = 1usize;
        let mut col = 1usize;
        for (idx, ch) in source.char_indices() {
            if idx >= self.start {
                break;
            }
            if ch == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        (line, col)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_covers_both() {
        let a = Span::new(3, 5);
        let b = Span::new(10, 12);
        assert_eq!(a.merge(b), Span::new(3, 12));
        assert_eq!(b.merge(a), Span::new(3, 12));
    }

    #[test]
    fn line_col_first_position() {
        assert_eq!(Span::new(0, 1).line_col("abc"), (1, 1));
    }

    #[test]
    fn line_col_multiline() {
        let src = "ab\ncd\nef";
        // Offset of 'd' is 4.
        assert_eq!(Span::new(4, 5).line_col(src), (2, 2));
    }

    #[test]
    fn line_col_counts_chars_not_bytes() {
        let src = "éxy";
        // 'é' is two bytes, so 'x' starts at byte offset 2 (the third byte) but
        // is the second character: the column must be 2, not 3.
        assert_eq!(Span::new(2, 3).line_col(src), (1, 2));
    }

    #[test]
    fn dummy_is_empty() {
        assert!(Span::dummy().is_empty());
    }
}
