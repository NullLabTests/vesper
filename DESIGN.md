# Vesper — Design

This document explains the architecture, data flow, invariants, and the
engineering decisions behind Vesper. It is part of the high-assurance process
artifacts for the project; see also [`REQUIREMENTS.md`](REQUIREMENTS.md) and
[`VERIFICATION.md`](VERIFICATION.md).

## 1. Goals and non-goals

**Goals**

* A complete, readable implementation of a small dynamically-typed language:
  lexer, parser, resolver, bytecode compiler, VM, and a tree-walking oracle.
* Correctness and auditability first: typed errors, span-precise diagnostics,
  explicit invariants, `unsafe` confined to a single audited module.
* Two independent execution back-ends that must agree on observable behavior.

**Non-goals**

* Raw performance parity with C interpreters (see §9).
* Cyclic garbage collection, JIT compilation, or static typing (§10 lists these
  as extension points).
* Features outside the specification (arrays, maps, modules, exceptions).

## 2. Pipeline and data flow

```text
+---------+   tokens   +--------+   AST    +----------+  distances  +-------------+
| lexer   | ─────────▶ | parser | ───────▶ | resolver | ──────────▶ | environment |
+---------+            +--------+          +----------+             | (tree-walk) |
     │                                                            +-------------+
     │                        AST                                    (oracle)
     └───────────────┬────────────┘
                     ▼
                +----------+   Function/Chunk   +---------+
                | compiler | ─────────────────▶ |   vm    |
                +----------+                    +---------+
```

Every stage consumes the previous stage's output and produces either a value or
a `Vec`/single `error::Error` carrying a `span::Span`. The tree-walker and the VM
share the lexer, parser, and AST but perform scope analysis independently
(§6).

### 2.1 Stage contracts

| Stage | Input | Output | Failure |
| --- | --- | --- | --- |
| `lexer::tokenize` | `&str` | `Vec<Token>` | `Vec<Error::Lexer>` |
| `parser::parse` | `&str` | `Vec<Stmt>` | `Vec<Error::Parse>` |
| `resolver::resolve` | `&[Stmt]` | `ResolveOutput { resolutions, errors }` | `Vec<Error::Compile>` |
| `compiler::compile` | `&[Stmt]`, source | `Rc<Function>` | `Vec<Error::Compile>` |
| `environment::interpret` | `&[Stmt]` | `String` (output) | `Error` |
| `vm::Vm::interpret` | `Rc<Function>` | `String` (output) | `Error` |

## 3. Lexer

The lexer is a hand-written scanner over `&str` with byte offsets (`Span`). It
tracks a `current` index and emits `token::Token { kind, lexeme, span, literal }`.
Comments (`//` and `/* */`) are skipped; numbers support fractions and
scientific notation; strings support `\t \n \r \" \\` escapes.

**Invariants**

* *Forward progress*: every `next_token` call advances the cursor on success and
  on error, so tokenization terminates. UTF-8 is decoded safely with
  `str::get(at..)`, never by slicing blindly; an invalid character consumes the
  remainder of its multibyte sequence and emits exactly one diagnostic.
* *No reads past the end*: all indexing is bounds-checked against the source
  length; the terminal `Eof` token is always produced.

## 4. AST and parser

`ast::Expr` and `ast::Stmt` are closed enums (the specification's guidance to
prefer enums over traits for closed sets). Every variant carries a `Span`;
recursive variants box their children. Nodes derive `Debug` and `Clone` for
tests.

The parser is recursive descent with precedence climbing for expressions and
synchronization-based error recovery at statement boundaries, so it reports
multiple independent errors per run rather than stopping at the first.

### 4.1 Grammar

```text
program     → declaration* EOF

declaration → classDecl | funDecl | varDecl | statement
classDecl   → "class" IDENT ( "<" IDENT )? "{" function* "}"
funDecl     → "fun" function
function    → IDENT "(" parameters? ")" block
varDecl     → "var" IDENT ( "=" expression )? ";"
parameters  → IDENT ( "," IDENT )*

statement   → exprStmt | forStmt | ifStmt | printStmt
            | returnStmt | whileStmt | breakStmt | continueStmt | block
exprStmt    → expression ";"
forStmt     → "for" "(" ( varDecl | exprStmt | ";" ) expression? ";" expression? ")" statement
ifStmt      → "if" "(" expression ")" statement ( "else" statement )?
printStmt   → "print" expression ";"
returnStmt  → "return" expression? ";"
whileStmt   → "while" "(" expression ")" statement
breakStmt   → "break" ";"
continueStmt→ "continue" ";"
block       → "{" declaration* "}"

expression  → assignment
assignment  → ( call "." )? IDENT "=" assignment | logic_or
logic_or    → logic_and ( "or" logic_and )*
logic_and   → equality ( "and" equality )*
equality    → comparison ( ( "!=" | "==" ) comparison )*
comparison  → term ( ( ">" | ">=" | "<" | "<=" ) term )*
term        → factor ( ( "-" | "+" ) factor )*
factor      → unary ( ( "/" | "*" ) unary )*
unary       → ( "!" | "-" ) unary | call
call        → primary ( "(" arguments? ")" | "." IDENT )*
arguments   → expression ( "," expression )*
primary     → "true" | "false" | "nil" | "this"
            | "super" "." IDENT | NUMBER | STRING | IDENT
            | "(" expression ")"
```

### 4.2 Error recovery

On a parse error the parser records the diagnostic and calls `synchronize`,
which discards tokens until a statement boundary (`;`, a declaration keyword, or
a terminator) and resumes. This yields useful multi-error output for tools while
keeping the happy path a straightforward recursive descent.

## 5. Bytecode format

### 5.1 `Chunk`

```rust
pub struct Chunk {
    pub code: Vec<u8>,          // opcodes and inline operands
    pub constants: Vec<Value>,  // de-duplicated constant pool
    lines: <run-length encoded>,// source line per byte
    spans: Vec<Span>,           // source span per byte (for diagnostics)
}
```

Line information is run-length encoded to keep the common case (many bytecodes
per line) compact, while `spans` retains the exact source range of each
instruction for runtime diagnostics.

### 5.2 Instruction encoding

* A `u8` operand is a constant-pool index, a local slot, an upvalue index, or an
  argument count.
* A `u16` operand is **big-endian** and represents a jump/loop offset. Big-endian
  was chosen so the disassembler prints offsets in reading order and `read_u16`
  is a trivial shift/or.

Jump semantics: `OP_JUMP`/`OP_JUMP_IF_FALSE` set `ip += offset` (the conditional
variant peeks and the compiler emits an explicit `OP_POP` on the fall-through
path); `OP_LOOP` sets `ip -= offset`. Set opcodes leave the assigned value on the
stack (clox convention), and `OP_SET_PROPERTY` pushes the value back so the
expression's value is preserved.

### 5.3 Opcodes

`OP_CONSTANT OP_NIL OP_TRUE OP_FALSE OP_POP OP_GET_LOCAL OP_SET_LOCAL
OP_GET_GLOBAL OP_DEFINE_GLOBAL OP_SET_GLOBAL OP_GET_UPVALUE OP_SET_UPVALUE
OP_GET_PROPERTY OP_SET_PROPERTY OP_GET_SUPER OP_EQUAL OP_GREATER OP_LESS OP_ADD
OP_SUBTRACT OP_MULTIPLY OP_DIVIDE OP_NOT OP_NEGATE OP_PRINT OP_JUMP
OP_JUMP_IF_FALSE OP_LOOP OP_CALL OP_INVOKE OP_SUPER_INVOKE OP_CLOSURE
OP_CLOSE_UPVALUE OP_RETURN OP_CLASS OP_INHERIT OP_METHOD`

The numeric opcode values are a stable contract between the compiler and the VM;
`OpCode::from_byte` and `OpCode::as_byte` must stay in sync (asserted by tests).

## 6. Scope analysis: why two engines

Lexical scoping is computed twice, on purpose:

* **`resolver.rs`** walks the AST and records, for each variable reference, the
  lexical distance to its declaration. It powers the tree-walker (which looks
  names up by distance into a chain of `Environment` frames) and produces
  semantic diagnostics (use of uninitialized variables, redeclaration in the same
  scope, `return` outside a function, invalid `this`/`super`).
* **`compiler.rs`** independently resolves locals to stack slots and captured
  variables to upvalue indices, clox-style, using a stack of function states.

The two are deliberately not shared: an error in one would not silently mask the
same error in the other, and agreement of the two back-ends over the fixture
corpus is a strong differential test (see requirement R4 and `VERIFICATION.md`).

## 7. Value representation and heap objects

```rust
pub enum Value { Nil, Bool(bool), Number(f64), Obj(Rc<Object>) }
```

* A **tagged enum + reference-counted heap** was chosen over a NaN-boxed or
  unboxed representation for clarity and safety. Numbers are inline; everything
  else is an `Rc<Object>`.
* `object::Object` is a closed enum of `Function`, `Closure`, `NativeFn`,
  `Upvalue`, `Class`, `Instance`, and `BoundMethod`. `Rc<str>` is used for
  strings so cloning a string value is cheap.
* Equality (`Value::equals`) follows the language spec: numbers compare with
  IEEE-754 `==` (so `NaN != NaN`), strings by contents, and heap objects by
  identity except where the spec says otherwise.

### 7.1 Memory management and safety

Memory is managed by **reference counting** (`Rc`/`RefCell`), not tracing GC.
This gives deterministic reclamation, no stop-the-world pauses, and a memory
model that is safe Rust everywhere except one audited module. `unsafe_code` is
`deny`d at the crate root; `src/stack.rs` (the interpreter's hot value stack) is
the sole module that opts back in with `#[allow(unsafe_code)]`, and it documents
the `Stack` invariants and the safety of every unchecked operation. The
documented limitation is that reference cycles are not reclaimed; the language as
specified has no way to create cycles between instances without going through
fields, and cycles are out of scope (§10).

`RefCell` borrow discipline is central to VM correctness. The hot borrows are
kept short and the code avoids holding a `Ref`/`RefMut` across a call back into
the VM. For example, instance field access borrows `instance.fields` only for
the duration of the read, and `close_upvalues` snapshots the open-upvalue list
into a local vector before mutating, precisely to avoid overlapping borrows.

## 8. Upvalues: open and closed

A closure captures variables, not values. Each captured cell is an
`Rc<RefCell<Upvalue>>` whose `location` is either:

* `UpvalueLocation::Stack(index)` — **open**: the cell still points at a slot in
  the VM value stack, so reads/writes see the live local; or
* `UpvalueLocation::Closed(value)` — **closed**: the variable has gone out of
  scope and its value was copied into the cell.

The VM keeps a list of **open upvalues** sorted by stack index. `OP_CLOSURE`
captures a local by finding an existing open upvalue for that slot or creating
one; capturing from the enclosing function copies the enclosing closure's
upvalue handle. `OP_CLOSE_UPVALUE` (emitted at scope exit) and `OP_RETURN`
(the helper `close_upvalues`) move every open cell at or above the relevant
index to the closed form. This is the classic clox algorithm, stated here as the
invariant: *an open upvalue's `index` always refers to a live stack slot; once
that slot is popped or aliased away, the upvalue is closed.*

## 9. Why a stack-based VM

* **Uniform, tiny encoding.** A stack machine's operands are implicit, so a
  single byte per instruction suffices; the disassembler and the compiler's jump
  patching are correspondingly simple and easy to audit.
* **Trivial frame discipline.** Locals live at fixed offsets from a frame base;
  pushes and pops are the only state transitions, which makes the stack-height
  invariants (asserted with `debug_assert!`) local and checkable.
* **A natural oracle pairing.** The tree-walker and stack VM differ enough in
  mechanism to be independently wrong, but agree trivially on the semantics of
  each opcode, making differential testing meaningful.

A **register-based VM** would reduce instruction count (and could be faster), but
at the cost of a wider encoding, more complex register allocation, and a much
harder correctness argument for an educational implementation. The specification
prioritizes clarity, so the stack VM wins.

### 9.1 Performance notes

* `OP_GET_GLOBAL`/`OP_SET_GLOBAL` hash string names in a `HashMap<String, Value>`
  (now with an FNV hasher rather than SipHash). Interning global names (pointer
  comparison, then a slot operand) is the obvious future win; the current design
  favors correctness and clarity. After the optimization passes recorded in
  `VERIFICATION.md`, global lookups are below 1% of instructions on the Fibonacci
  workload.
* `fib(35)` runs in roughly 8.5 s in a release build on the development machine, a
  2018 15 W ULV laptop, versus the R8 target of under 5 s — see `VERIFICATION.md`
  for the honest measurement and the completed optimization passes. The value
  stack and call frames are pre-sized to avoid reallocation in the common case.

## 10. Future extension points

* **Cyclic GC.** Replace or supplement `Rc` with a tracing collector
  (`Rc` + mark-sweep, or an arena with indices) and add a `Value`-level root set.
  The `Object` enum is closed and centralized, which localizes this change.
* **Static types.** The resolver already computes a scope model; adding a type
  checker as a new pass over the AST (and a second use of `Error::Compile`) fits
  the pipeline without touching the VM.
* **JIT / tiered execution.** The bytecode is a clean IR; a baseline compiler
  could translate chunks to native code behind the same `Vm` interface.
* **String interning and inline caches.** `OP_GET_GLOBAL` and method dispatch
  (`OP_INVOKE`) are the natural places for interning and per-site caches.
* **Register-based backend.** A second compiler target could emit register
  bytecode, reusing the same AST and diagnostics.

## 11. Invariants (summary)

1. Every token, AST node, chunk byte, and error carries a valid source span
   (`Span::dummy` only for synthesized nodes).
2. The lexer always makes forward progress and always terminates with `Eof`.
3. The parser either returns a well-formed AST or a non-empty error list; it
   never panics on arbitrary input (property-tested).
4. Constant-pool indices are `< MAX_CONSTANTS`; local slots are `< MAX_LOCALS`;
   upvalues are `< MAX_UPVALUES`; parameters are `<= MAX_PARAMS`.
5. At any instruction boundary the value stack holds exactly the locals of all
   active frames plus the temporaries of the executing frame.
6. An open upvalue's stack index always refers to a live slot.
7. The VM and the tree-walker produce identical `print` output for every
   program neither rejects (differential testing).
8. The crate's only `unsafe` lives in `src/stack.rs`, whose unchecked operations
   uphold its documented invariants (every other module is compiled with
   `unsafe_code` denied).
