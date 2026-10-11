//! Bytecode container and disassembler.
//!
//! A [`Chunk`] is the unit of compiled code: a byte vector, a constant pool, and
//! parallel metadata for diagnostics (one line number and one [`Span`] per
//! byte). Storing a span per byte lets the VM raise runtime errors with the
//! exact source range of the faulting instruction at negligible cost for the
//! program sizes Vesper targets.
//!
//! # Invariants
//!
//! * `lines.len() == code.len()` and `spans.len() == code.len()` after every
//!   `write*` call.
//! * `constants.len() <= MAX_CONSTANTS` — enforced by [`Chunk::add_constant`].
//! * Constant indices stored in operands are valid indices into `constants`.

use crate::error::Error;
use crate::object::{Function, Object};
use crate::opcode::OpCode;
use crate::span::Span;
use crate::value::Value;
use std::fmt::Write as _;

/// Maximum number of constants in a single chunk.
///
/// Constant operands are one byte, so the pool cannot exceed 256 entries. This
/// limit is generous for hand-written programs; exceeding it is a compile error
/// rather than silent truncation.
pub const MAX_CONSTANTS: usize = 256;

/// Compiled bytecode plus its constant pool and diagnostic metadata.
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    /// The bytecode.
    pub code: Vec<u8>,
    /// The constant pool.
    pub constants: Vec<Value>,
    /// Source line for each byte in `code`.
    pub lines: Vec<usize>,
    /// Source span for each byte in `code`.
    pub spans: Vec<Span>,
}

impl Chunk {
    /// Creates an empty chunk.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an opcode with its diagnostic location.
    pub fn write(&mut self, op: OpCode, line: usize, span: Span) {
        self.write_byte(op.as_byte(), line, span);
    }

    /// Appends a raw byte with its diagnostic location.
    pub fn write_byte(&mut self, byte: u8, line: usize, span: Span) {
        self.code.push(byte);
        self.lines.push(line);
        self.spans.push(span);
    }

    /// Appends a big-endian `u16` operand.
    pub fn write_u16(&mut self, value: u16, line: usize, span: Span) {
        self.write_byte((value >> 8) as u8, line, span);
        self.write_byte((value & 0xff) as u8, line, span);
    }

    /// Adds a constant, returning its one-byte index.
    ///
    /// Identical constants are de-duplicated so a hot loop that uses the same
    /// numeral or string repeatedly occupies a single pool slot.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Compile`] if the pool is full ([`MAX_CONSTANTS`]).
    pub fn add_constant(&mut self, value: Value, span: Span) -> Result<u8, Error> {
        if let Some(idx) = self.constants.iter().position(|c| c.equals(&value)) {
            return Ok(idx as u8);
        }
        if self.constants.len() >= MAX_CONSTANTS {
            return Err(Error::compile(
                format!("constant pool overflow (max {MAX_CONSTANTS} constants)"),
                span,
            ));
        }
        self.constants.push(value);
        Ok((self.constants.len() - 1) as u8)
    }

    /// Returns the constant at `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of range; the compiler guarantees validity via
    /// [`Chunk::add_constant`], so a panic here indicates a compiler bug.
    #[must_use]
    pub fn constant(&self, index: u8) -> &Value {
        &self.constants[index as usize]
    }

    /// Returns the source line for byte `offset`, or 0 if out of range.
    #[must_use]
    pub fn line(&self, offset: usize) -> usize {
        self.lines.get(offset).copied().unwrap_or(0)
    }

    /// Returns the source span for byte `offset`, or a dummy span if out of
    /// range.
    #[must_use]
    pub fn span(&self, offset: usize) -> Span {
        self.spans.get(offset).copied().unwrap_or_else(Span::dummy)
    }

    /// Reads a big-endian `u16` from `code` starting at `at`.
    #[must_use]
    pub fn read_u16(code: &[u8], at: usize) -> u16 {
        let hi = u16::from(code.get(at).copied().unwrap_or(0));
        let lo = u16::from(code.get(at + 1).copied().unwrap_or(0));
        (hi << 8) | lo
    }
}

/// Disassembles an entire chunk, appending to `out` with a header `name`.
pub fn disassemble_chunk(chunk: &Chunk, name: &str, out: &mut String) {
    let _ = writeln!(out, "== {name} ==");
    let mut offset = 0;
    while offset < chunk.code.len() {
        offset = disassemble_instruction(chunk, offset, out);
    }
}

/// Disassembles `function` and, recursively, every function nested in its
/// constant pool.
///
/// Nested functions are compiled into `OP_CLOSURE` constants rather than inlined
/// into the enclosing chunk, so a full listing of a program must descend into
/// the constant pool.
pub fn disassemble_function(function: &Function, out: &mut String) {
    disassemble_chunk(&function.chunk, &function.name, out);
    for constant in &function.chunk.constants {
        if let Value::Obj(obj) = constant {
            if let Object::Function(nested) = &**obj {
                let _ = writeln!(out);
                disassemble_function(nested, out);
            }
        }
    }
}

/// Disassembles one instruction, appending to `out`, and returns the offset of
/// the next instruction.
pub fn disassemble_instruction(chunk: &Chunk, offset: usize, out: &mut String) -> usize {
    let _ = write!(out, "{offset:04} ");
    let line = chunk.line(offset);
    if offset > 0 && chunk.line(offset - 1) == line {
        let _ = write!(out, "   | ");
    } else {
        let _ = write!(out, "{line:4} ");
    }

    let Some(op) = OpCode::from_byte(chunk.code[offset]) else {
        let _ = writeln!(out, "<unknown byte {:#04x}>", chunk.code[offset]);
        return offset + 1;
    };

    match op {
        OpCode::Constant
        | OpCode::GetGlobal
        | OpCode::DefineGlobal
        | OpCode::SetGlobal
        | OpCode::GetProperty
        | OpCode::SetProperty
        | OpCode::GetSuper
        | OpCode::Class
        | OpCode::Method => {
            let idx = chunk.code[offset + 1];
            let _ = writeln!(
                out,
                "{:<20} {:4} '{}'",
                op.name(),
                idx,
                describe_constant(chunk, idx)
            );
            offset + 2
        }
        OpCode::GetLocal | OpCode::SetLocal | OpCode::GetUpvalue | OpCode::SetUpvalue => {
            // These operands are slot/upvalue indices, not constant indices.
            let slot = chunk.code[offset + 1];
            let _ = writeln!(out, "{:<20} {:4}", op.name(), slot);
            offset + 2
        }
        OpCode::Call => {
            let count = chunk.code[offset + 1];
            let _ = writeln!(out, "{:<20} {:4}", op.name(), count);
            offset + 2
        }
        OpCode::Invoke | OpCode::SuperInvoke => {
            let idx = chunk.code[offset + 1];
            let count = chunk.code[offset + 2];
            let _ = writeln!(
                out,
                "{:<20} ({} args) {:4} '{}'",
                op.name(),
                count,
                idx,
                describe_constant(chunk, idx)
            );
            offset + 3
        }
        OpCode::Jump | OpCode::JumpIfFalse => {
            let jump = Chunk::read_u16(&chunk.code, offset + 1);
            let _ = writeln!(
                out,
                "{:<20} {:4} -> {}",
                op.name(),
                jump,
                offset + 3 + jump as usize
            );
            offset + 3
        }
        OpCode::Loop => {
            let jump = Chunk::read_u16(&chunk.code, offset + 1);
            let _ = writeln!(
                out,
                "{:<20} {:4} -> {}",
                op.name(),
                jump,
                offset + 3 - jump as usize
            );
            offset + 3
        }
        OpCode::Closure => {
            let idx = chunk.code[offset + 1];
            let name = describe_constant(chunk, idx);
            let _ = writeln!(out, "{:<20} {:4} '{name}'", op.name(), idx);
            let upvalue_count = closure_upvalue_count(chunk, idx);
            let mut cur = offset + 2;
            for _ in 0..upvalue_count {
                let is_local = chunk.code[cur];
                let index = chunk.code[cur + 1];
                let kind = if is_local == 1 { "local" } else { "upvalue" };
                let _ = writeln!(out, "{cur:04}      |                     {kind} {index}");
                cur += 2;
            }
            cur
        }
        _ => {
            let _ = writeln!(out, "{}", op.name());
            offset + 1
        }
    }
}

/// Describes a constant for the disassembler.
///
/// Returns `<invalid>` rather than panicking if `index` is out of range, so the
/// disassembler can still render malformed or interpreter-only operands.
fn describe_constant(chunk: &Chunk, index: u8) -> String {
    chunk
        .constants
        .get(index as usize)
        .map_or_else(|| "<invalid>".to_string(), Value::to_display)
}

/// Determines the number of upvalue descriptors that follow an `OP_CLOSURE`,
/// by inspecting the referenced function constant. Returns 0 if the constant is
/// not a function.
fn closure_upvalue_count(chunk: &Chunk, index: u8) -> usize {
    match chunk.constants.get(index as usize) {
        Some(Value::Obj(o)) => match &**o {
            Object::Function(f) => f.upvalue_count,
            Object::Closure(c) => c.function.upvalue_count,
            _ => 0,
        },
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_keeps_metadata_parallel() {
        let mut chunk = Chunk::new();
        chunk.write(OpCode::Nil, 1, Span::new(0, 1));
        chunk.write(OpCode::Return, 2, Span::new(2, 3));
        assert_eq!(chunk.code.len(), 2);
        assert_eq!(chunk.lines.len(), 2);
        assert_eq!(chunk.spans.len(), 2);
        assert_eq!(chunk.line(0), 1);
        assert_eq!(chunk.span(1), Span::new(2, 3));
    }

    #[test]
    fn constants_are_deduplicated() {
        let mut chunk = Chunk::new();
        let a = chunk
            .add_constant(Value::Number(1.0), Span::dummy())
            .unwrap();
        let b = chunk
            .add_constant(Value::Number(1.0), Span::dummy())
            .unwrap();
        let c = chunk
            .add_constant(Value::Number(2.0), Span::dummy())
            .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(chunk.constants.len(), 2);
    }

    #[test]
    fn constant_pool_overflow_is_error() {
        let mut chunk = Chunk::new();
        for i in 0..MAX_CONSTANTS {
            chunk
                .add_constant(Value::Number(i as f64), Span::dummy())
                .unwrap();
        }
        let err = chunk.add_constant(Value::string("overflow"), Span::dummy());
        assert!(err.is_err());
    }

    #[test]
    fn u16_roundtrip() {
        let mut chunk = Chunk::new();
        chunk.write_u16(0xABCD, 1, Span::dummy());
        assert_eq!(Chunk::read_u16(&chunk.code, 0), 0xABCD);
    }

    #[test]
    fn disassembles_simple_program() {
        let mut chunk = Chunk::new();
        let k = chunk
            .add_constant(Value::Number(1.5), Span::dummy())
            .unwrap();
        chunk.write(OpCode::Constant, 0, Span::dummy());
        chunk.write_byte(k, 0, Span::dummy());
        chunk.write(OpCode::Return, 0, Span::dummy());
        let mut out = String::new();
        disassemble_chunk(&chunk, "test", &mut out);
        assert!(out.contains("OP_CONSTANT"));
        assert!(out.contains("1.5"));
        assert!(out.contains("OP_RETURN"));
    }
}
