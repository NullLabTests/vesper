//! Runtime values.
//!
//! A [`Value`] is either an immediate (`nil`, `bool`, `number`) or a reference
//! to a heap [`Object`]. Numbers are 64-bit IEEE-754 doubles; booleans and
//! `nil` are immediate. Strings and all user-defined constructs live on the
//! heap behind [`Rc`].
//!
//! # Equality
//!
//! [`Value::equals`] implements the language's `==` semantics:
//!
//! * `nil == nil`, booleans by value, numbers by IEEE-754 `==` (so `NaN != NaN`,
//!   matching the underlying primitive).
//! * Objects compare structurally for strings and by identity otherwise.

use crate::object::Object;
use std::fmt;
use std::rc::Rc;

/// A Vesper runtime value.
#[derive(Clone)]
pub enum Value {
    /// The `nil` value.
    Nil,
    /// A boolean.
    Bool(bool),
    /// A floating-point number.
    Number(f64),
    /// A heap object.
    Obj(Rc<Object>),
}

impl Value {
    /// Creates a string value.
    #[must_use]
    pub fn string(s: impl AsRef<str>) -> Self {
        Self::Obj(Rc::new(Object::string(s)))
    }

    /// Returns `true` if the value is falsy (`nil` or `false`).
    #[must_use]
    pub const fn is_falsey(&self) -> bool {
        matches!(self, Self::Nil | Self::Bool(false))
    }

    /// A short, stable type name for diagnostics.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Nil => "nil",
            Self::Bool(_) => "bool",
            Self::Number(_) => "number",
            Self::Obj(o) => o.type_name(),
        }
    }

    /// Extracts the numeric payload, if this is a number.
    #[must_use]
    pub const fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Extracts the string payload, if this is a string object.
    #[must_use]
    pub fn as_string(&self) -> Option<Rc<str>> {
        match self {
            Self::Obj(o) => o.as_string(),
            _ => None,
        }
    }

    /// Returns the heap object, if any.
    #[must_use]
    pub const fn as_obj(&self) -> Option<&Rc<Object>> {
        match self {
            Self::Obj(o) => Some(o),
            _ => None,
        }
    }

    /// Language-level `==`.
    #[must_use]
    pub fn equals(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Number(a), Self::Number(b)) => a == b,
            (Self::Obj(a), Self::Obj(b)) => a.equals(b),
            _ => false,
        }
    }

    /// Renders the value as `print` would.
    #[must_use]
    pub fn to_display(&self) -> String {
        match self {
            Self::Nil => "nil".to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Number(n) => format_number(*n),
            Self::Obj(o) => format!("{o}"),
        }
    }
}

/// Formats a number the way the language prints it: integral values without a
/// trailing `.0`, everything else via the default float formatter.
#[must_use]
pub fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => f.write_str("nil"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Number(n) => f.write_str(&format_number(*n)),
            Self::Obj(o) => o.display(f),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nil => f.write_str("Nil"),
            Self::Bool(b) => write!(f, "Bool({b})"),
            Self::Number(n) => write!(f, "Number({})", format_number(*n)),
            Self::Obj(o) => write!(f, "Obj({})", o.type_name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falsey_values() {
        assert!(Value::Nil.is_falsey());
        assert!(Value::Bool(false).is_falsey());
        assert!(!Value::Bool(true).is_falsey());
        assert!(!Value::Number(0.0).is_falsey());
        assert!(!Value::string("").is_falsey());
    }

    #[test]
    fn equality_semantics() {
        assert!(Value::Nil.equals(&Value::Nil));
        assert!(Value::Number(1.0).equals(&Value::Number(1.0)));
        assert!(!Value::Number(1.0).equals(&Value::string("1")));
        assert!(Value::string("a").equals(&Value::string("a")));
    }

    #[test]
    fn nan_is_not_equal_to_itself() {
        assert!(!Value::Number(f64::NAN).equals(&Value::Number(f64::NAN)));
    }

    #[test]
    fn number_formatting() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(3.5), "3.5");
        assert_eq!(format_number(-0.0), "0");
    }

    #[test]
    fn display_of_objects() {
        assert_eq!(Value::string("hi").to_display(), "hi");
        assert_eq!(Value::Nil.to_display(), "nil");
    }
}
