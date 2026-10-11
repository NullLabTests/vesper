//! Bytecode instruction set.
//!
//! Each [`OpCode`] is a single byte in a [`crate::chunk::Chunk`]. Operands, if
//! any, follow the opcode inline. The encoding is:
//!
//! * `u8` operand: a constant-pool index, a local slot, an upvalue index, or an
//!   argument count.
//! * `u16` operand (big-endian): a jump/loop offset or a compound index.
//!
//! Big-endian `u16` operands are chosen so the disassembler prints them in
//! reading order and so that `read_u16` is a simple shift/or.

/// A bytecode opcode.
///
/// The discriminant values are part of the on-disk/interpreter contract;
/// [`OpCode::from_byte`] must stay in sync with [`OpCode::as_byte`].
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpCode {
    Constant,
    Nil,
    True,
    False,
    Pop,
    GetLocal,
    SetLocal,
    GetGlobal,
    DefineGlobal,
    SetGlobal,
    GetUpvalue,
    SetUpvalue,
    GetProperty,
    SetProperty,
    GetSuper,
    Equal,
    Greater,
    Less,
    Add,
    Subtract,
    Multiply,
    Divide,
    Not,
    Negate,
    Print,
    Jump,
    JumpIfFalse,
    Loop,
    Call,
    Invoke,
    SuperInvoke,
    Closure,
    CloseUpvalue,
    Return,
    Class,
    Inherit,
    Method,
}

impl OpCode {
    /// The byte encoding of this opcode.
    #[must_use]
    pub const fn as_byte(self) -> u8 {
        match self {
            Self::Constant => 0,
            Self::Nil => 1,
            Self::True => 2,
            Self::False => 3,
            Self::Pop => 4,
            Self::GetLocal => 5,
            Self::SetLocal => 6,
            Self::GetGlobal => 7,
            Self::DefineGlobal => 8,
            Self::SetGlobal => 9,
            Self::GetUpvalue => 10,
            Self::SetUpvalue => 11,
            Self::GetProperty => 12,
            Self::SetProperty => 13,
            Self::GetSuper => 14,
            Self::Equal => 15,
            Self::Greater => 16,
            Self::Less => 17,
            Self::Add => 18,
            Self::Subtract => 19,
            Self::Multiply => 20,
            Self::Divide => 21,
            Self::Not => 22,
            Self::Negate => 23,
            Self::Print => 24,
            Self::Jump => 25,
            Self::JumpIfFalse => 26,
            Self::Loop => 27,
            Self::Call => 28,
            Self::Invoke => 29,
            Self::SuperInvoke => 30,
            Self::Closure => 31,
            Self::CloseUpvalue => 32,
            Self::Return => 33,
            Self::Class => 34,
            Self::Inherit => 35,
            Self::Method => 36,
        }
    }

    /// Decodes a byte into an opcode, or `None` if it is not a valid opcode.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::Constant,
            1 => Self::Nil,
            2 => Self::True,
            3 => Self::False,
            4 => Self::Pop,
            5 => Self::GetLocal,
            6 => Self::SetLocal,
            7 => Self::GetGlobal,
            8 => Self::DefineGlobal,
            9 => Self::SetGlobal,
            10 => Self::GetUpvalue,
            11 => Self::SetUpvalue,
            12 => Self::GetProperty,
            13 => Self::SetProperty,
            14 => Self::GetSuper,
            15 => Self::Equal,
            16 => Self::Greater,
            17 => Self::Less,
            18 => Self::Add,
            19 => Self::Subtract,
            20 => Self::Multiply,
            21 => Self::Divide,
            22 => Self::Not,
            23 => Self::Negate,
            24 => Self::Print,
            25 => Self::Jump,
            26 => Self::JumpIfFalse,
            27 => Self::Loop,
            28 => Self::Call,
            29 => Self::Invoke,
            30 => Self::SuperInvoke,
            31 => Self::Closure,
            32 => Self::CloseUpvalue,
            33 => Self::Return,
            34 => Self::Class,
            35 => Self::Inherit,
            36 => Self::Method,
            _ => return None,
        })
    }

    /// The mnemonic used by the disassembler.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Constant => "OP_CONSTANT",
            Self::Nil => "OP_NIL",
            Self::True => "OP_TRUE",
            Self::False => "OP_FALSE",
            Self::Pop => "OP_POP",
            Self::GetLocal => "OP_GET_LOCAL",
            Self::SetLocal => "OP_SET_LOCAL",
            Self::GetGlobal => "OP_GET_GLOBAL",
            Self::DefineGlobal => "OP_DEFINE_GLOBAL",
            Self::SetGlobal => "OP_SET_GLOBAL",
            Self::GetUpvalue => "OP_GET_UPVALUE",
            Self::SetUpvalue => "OP_SET_UPVALUE",
            Self::GetProperty => "OP_GET_PROPERTY",
            Self::SetProperty => "OP_SET_PROPERTY",
            Self::GetSuper => "OP_GET_SUPER",
            Self::Equal => "OP_EQUAL",
            Self::Greater => "OP_GREATER",
            Self::Less => "OP_LESS",
            Self::Add => "OP_ADD",
            Self::Subtract => "OP_SUBTRACT",
            Self::Multiply => "OP_MULTIPLY",
            Self::Divide => "OP_DIVIDE",
            Self::Not => "OP_NOT",
            Self::Negate => "OP_NEGATE",
            Self::Print => "OP_PRINT",
            Self::Jump => "OP_JUMP",
            Self::JumpIfFalse => "OP_JUMP_IF_FALSE",
            Self::Loop => "OP_LOOP",
            Self::Call => "OP_CALL",
            Self::Invoke => "OP_INVOKE",
            Self::SuperInvoke => "OP_SUPER_INVOKE",
            Self::Closure => "OP_CLOSURE",
            Self::CloseUpvalue => "OP_CLOSE_UPVALUE",
            Self::Return => "OP_RETURN",
            Self::Class => "OP_CLASS",
            Self::Inherit => "OP_INHERIT",
            Self::Method => "OP_METHOD",
        }
    }

    /// Number of operand bytes that follow this opcode.
    ///
    /// [`OpCode::Closure`] is variable-length (one constant index plus two bytes
    /// per captured upvalue); it is handled specially by the disassembler and
    /// returns `1` here.
    #[must_use]
    pub const fn operand_len(self) -> usize {
        match self {
            Self::Constant
            | Self::GetLocal
            | Self::SetLocal
            | Self::GetGlobal
            | Self::DefineGlobal
            | Self::SetGlobal
            | Self::GetUpvalue
            | Self::SetUpvalue
            | Self::GetProperty
            | Self::SetProperty
            | Self::GetSuper
            | Self::Call
            | Self::Class
            | Self::Method => 1,
            Self::Jump | Self::JumpIfFalse | Self::Loop | Self::Invoke | Self::SuperInvoke => 2,
            Self::Closure => 1,
            _ => 0,
        }
    }

    /// Returns `true` if this opcode's `u16` operand is a jump/loop offset.
    #[must_use]
    pub const fn is_jump(self) -> bool {
        matches!(self, Self::Jump | Self::JumpIfFalse | Self::Loop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_roundtrip_for_all_opcodes() {
        // Exhaustively check that as_byte/from_byte are inverse over 0..=255.
        for byte in 0u8..=255 {
            if let Some(op) = OpCode::from_byte(byte) {
                assert_eq!(op.as_byte(), byte);
            }
        }
    }

    #[test]
    fn invalid_byte_is_none() {
        assert!(OpCode::from_byte(255).is_none());
    }

    #[test]
    fn jump_classification() {
        assert!(OpCode::Jump.is_jump());
        assert!(OpCode::JumpIfFalse.is_jump());
        assert!(OpCode::Loop.is_jump());
        assert!(!OpCode::Add.is_jump());
    }

    #[test]
    fn operand_lengths() {
        assert_eq!(OpCode::Constant.operand_len(), 1);
        assert_eq!(OpCode::Jump.operand_len(), 2);
        assert_eq!(OpCode::Add.operand_len(), 0);
    }
}
