//! Read-eval-print loop.
//!
//! The REPL keeps a single [`Vm`] alive across inputs so global variables and
//! function definitions persist. Multi-line input is supported by scanning the
//! buffer for balanced delimiters and terminated strings; an incomplete entry
//! continues on the next line with a `.. ` prompt.

use crate::error::Error;
use crate::vm::Vm;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

/// The REPL state: a persistent VM plus the line editor.
pub struct Repl {
    vm: Vm,
    editor: DefaultEditor,
}

impl Repl {
    /// Creates a REPL with a fresh VM and line editor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the terminal cannot be initialised.
    pub fn new() -> Result<Self, Error> {
        let editor = DefaultEditor::new().map_err(|e| Error::Io(e.to_string()))?;
        Ok(Self {
            vm: Vm::new(),
            editor,
        })
    }

    /// Runs the interactive loop until EOF (Ctrl-D) or a fatal I/O error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if reading from the terminal fails unrecoverably.
    pub fn run(&mut self) -> Result<(), Error> {
        println!("Vesper REPL. Type `quit` or press Ctrl-D to exit.");
        loop {
            let mut buffer = String::new();
            loop {
                let prompt = if buffer.is_empty() { ">> " } else { ".. " };
                match self.editor.readline(prompt) {
                    Ok(line) => {
                        buffer.push_str(&line);
                        buffer.push('\n');
                        if is_complete(&buffer) {
                            break;
                        }
                    }
                    Err(ReadlineError::Interrupted) => {
                        // Ctrl-C abandons the current (possibly multi-line) entry.
                        buffer.clear();
                        break;
                    }
                    Err(ReadlineError::Eof) => {
                        println!();
                        return Ok(());
                    }
                    Err(e) => return Err(Error::Io(e.to_string())),
                }
            }

            let trimmed = buffer.trim();
            if trimmed.is_empty() {
                continue;
            }
            if matches!(trimmed, "quit" | "exit") {
                return Ok(());
            }
            let _ = self.editor.add_history_entry(trimmed);
            self.eval(&buffer);
        }
    }

    /// Compiles and runs one complete entry, printing output or diagnostics.
    fn eval(&mut self, source: &str) {
        match crate::compile_source(source) {
            Ok(function) => match self.vm.interpret(function) {
                Ok(output) => print!("{output}"),
                Err(error) => eprint!("{}", error.render("<repl>", source)),
            },
            Err(error) => eprint!("{}", error.render("<repl>", source)),
        }
    }
}

/// Returns `true` when `source` ends at a statement boundary.
///
/// A buffer is complete when all `()[]{}` are balanced, no string is open, and
/// no block comment is open.
#[must_use]
pub fn is_complete(source: &str) -> bool {
    let mut depth: i64 = 0;
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '/' if chars.peek() == Some(&'/') => {
                // Line comment: skip to end of line.
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut closed = false;
                let mut prev = '\0';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        closed = true;
                        break;
                    }
                    prev = c;
                }
                if !closed {
                    return false;
                }
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    depth <= 0 && !in_string
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_input_is_complete() {
        assert!(is_complete("print 1 + 2;"));
        assert!(is_complete("fun f() { return 1; }"));
    }

    #[test]
    fn open_brace_is_incomplete() {
        assert!(!is_complete("fun f() {"));
        assert!(!is_complete("if (true) {"));
    }

    #[test]
    fn open_string_is_incomplete() {
        assert!(!is_complete("print \"hello"));
    }

    #[test]
    fn closed_string_is_complete() {
        assert!(is_complete("print \"hello\";"));
    }

    #[test]
    fn block_comment_must_close() {
        assert!(!is_complete("/* not closed"));
        assert!(is_complete("/* closed */ print 1;"));
    }
}
