//! A capacity-managed [`Value`] stack whose hot operations are unchecked.
//!
//! # Why this module is allowed to use `unsafe`
//!
//! `unsafe_code` is denied for the whole crate except here. This is the single
//! hottest data structure in the interpreter: on recursive benchmarks the value
//! stack sees tens of millions of pushes, pops, peeks and indexed loads, and the
//! safe `Vec` operations spent a measurable share of retired instructions on
//! bounds checks and `Option` plumbing that the VM's own invariants already make
//! redundant. The specification permits a module-scoped `allow(unsafe_code)`
//! when it is justified and the operations are small and auditable, which is the
//! case here.
//!
//! # Invariants
//!
//! [`Stack`] owns a `Vec<Value>` and maintains exactly three invariants:
//!
//! 1. `data.len()` is the number of initialised, owned elements.
//! 2. `data.capacity() >= data.len()` (guaranteed by `Vec`).
//! 3. Every element with index `>= data.len()` is uninitialised or logically
//!    moved-from, and is never read or dropped while `data.len()` excludes it.
//!
//! Every `unsafe` block below relies only on these invariants and the stated
//! caller precondition; no other assumptions are made about `Value`.

#![allow(unsafe_code)]

use crate::value::Value;
use std::slice::Iter;

/// A stack of [`Value`]s with allocation-accounted, unchecked accessors.
///
/// The API is deliberately small: only the operations the interpreter needs, so
/// the safety argument stays auditable. All indexed accessors carry a
/// precondition that the caller must uphold.
pub struct Stack {
    data: Vec<Value>,
}

impl Stack {
    /// Creates a stack with room for `capacity` values before the first grow.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    /// Number of values currently on the stack.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the stack is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Pushes `value` onto the top of the stack.
    #[inline]
    pub fn push(&mut self, value: Value) {
        if self.data.len() == self.data.capacity() {
            // Cold path: grow so that `capacity() > len()`. Growth preserves the
            // address-visible invariants of the unchecked path below.
            self.data.reserve(1);
        }
        // SAFETY: the branch above guarantees `len < capacity`, so the write is
        // inside the allocation. The slot at `len` is uninitialised per
        // invariant 3 (it has never been read or dropped); writing `value`
        // initialises it, and `set_len` promotes it into the owned range.
        unsafe {
            let len = self.data.len();
            self.data.as_mut_ptr().add(len).write(value);
            self.data.set_len(len + 1);
        }
    }

    /// Pops the top value.
    ///
    /// Precondition: the stack is non-empty.
    #[inline]
    pub fn pop(&mut self) -> Value {
        debug_assert!(!self.data.is_empty(), "value stack underflow");
        let len = self.data.len();
        // SAFETY: the precondition gives `len >= 1`, so `len - 1` is an
        // initialised, owned element (invariant 1). Shrinking the `Vec` first
        // removes it from the owned range so it is not dropped by the `Vec`;
        // `read` then moves it out exactly once. This element becomes
        // logically moved-from, preserving invariant 3 for the tail.
        unsafe {
            self.data.set_len(len - 1);
            self.data.as_ptr().add(len - 1).read()
        }
    }

    /// Returns the value `distance` below the top (`0` is the top).
    ///
    /// Precondition: `distance < len`.
    #[inline]
    #[must_use]
    pub fn peek(&self, distance: usize) -> &Value {
        debug_assert!(distance < self.data.len(), "peek past bottom of stack");
        // SAFETY: the precondition makes `len - 1 - distance` a valid index into
        // the initialised owned range, which `get_unchecked` returns by
        // reference without a bounds check.
        unsafe { self.data.get_unchecked(self.data.len() - 1 - distance) }
    }

    /// Returns the value at `index`.
    ///
    /// Precondition: `index < len`.
    #[inline]
    #[must_use]
    pub fn get(&self, index: usize) -> &Value {
        debug_assert!(index < self.data.len(), "stack index out of range");
        // SAFETY: the precondition places `index` in the initialised owned range.
        unsafe { self.data.get_unchecked(index) }
    }

    /// Overwrites the value at `index`.
    ///
    /// Precondition: `index < len`.
    #[inline]
    pub fn set(&mut self, index: usize, value: Value) {
        debug_assert!(index < self.data.len(), "stack index out of range");
        // SAFETY: the precondition places `index` in the initialised owned range,
        // so the slot is a valid, owned `Value`. Assigning through the unchecked
        // reference drops the previous value and stores `value` exactly once.
        unsafe {
            *self.data.get_unchecked_mut(index) = value;
        }
    }

    /// Drops all but the first `new_len` values.
    ///
    /// Precondition: `new_len <= len`.
    #[inline]
    pub fn truncate(&mut self, new_len: usize) {
        self.data.truncate(new_len);
    }

    /// Clones the values from `start` to the top into a fresh [`Vec`].
    ///
    /// Precondition: `start <= len`.
    #[must_use]
    pub fn tail_from(&self, start: usize) -> Vec<Value> {
        debug_assert!(start <= self.data.len(), "slice start past top of stack");
        // SAFETY: the precondition makes `start..len` a subslice of the
        // initialised owned range.
        unsafe { self.data.get_unchecked(start..).to_vec() }
    }

    /// Iterates over the values from bottom to top.
    pub fn iter(&self) -> Iter<'_, Value> {
        self.data.iter()
    }
}
