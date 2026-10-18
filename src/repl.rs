//! Interactive read-eval-print loop for Takolisp.
//!
//! This module is only available with the `repl` feature.

use std::io::IsTerminal;

use anyhow::Result;
use rustyline::{DefaultEditor, error::ReadlineError};

use crate::{
    ModuleSource, Val, Vm,
    error::{ErrorPalette, VmErrorConfig},
};

/// Terminal colors used by the REPL and by rendered VM errors.
#[derive(Copy, Clone, Debug)]
pub struct Palette {
    prompt: &'static str,
    output: &'static str,
    error: &'static str,
    location: &'static str,
    reset: &'static str,
}

impl Palette {
    /// Palette using ANSI terminal colors.
    pub const COLOR: Self = Self {
        prompt: "\x1b[35m",   // magenta
        output: "\x1b[36m",   // cyan
        error: "\x1b[31m",    // red
        location: "\x1b[34m", // blue
        reset: "\x1b[0m",     // reset
    };

    /// Palette without any escape sequences.
    pub const PLAIN: Self = Self {
        prompt: "",
        output: "",
        error: "",
        location: "",
        reset: "",
    };

    /// Prompt to display, including any color escape sequences.
    pub fn prompt(&self) -> String {
        // Rustyline uses these markers to exclude terminal control sequences
        // from its prompt width and cursor calculations.
        format!("{}λ{} ", self.prompt, self.reset)
    }

    /// Colors to use when displaying a VM error.
    pub fn error(&self) -> ErrorPalette {
        ErrorPalette {
            error: self.error,
            location: self.location,
            reset: self.reset,
        }
    }
}

impl Default for Palette {
    /// Returns [`Palette::COLOR`] when both stdout and stderr are terminals,
    /// and [`Palette::PLAIN`] otherwise.
    fn default() -> Self {
        if std::io::stdout().is_terminal() && std::io::stderr().is_terminal() {
            Self::COLOR
        } else {
            Self::PLAIN
        }
    }
}

/// Interactive read-eval-print loop.
///
/// The result of each evaluated line is bound to `$1`, `$2`, and so on in the
/// REPL module, and is printed in cyan. Errors are printed to stderr.
///
/// ```no_run
/// takolisp::repl::Repl::new().run().unwrap();
/// ```
pub struct Repl {
    palette: Palette,
}

impl Repl {
    /// Create a REPL using the default palette.
    pub fn new() -> Self {
        Self {
            palette: Palette::default(),
        }
    }

    /// Use a specific color palette.
    pub fn with_palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }

    /// Run the REPL until end of input or an interrupt.
    pub fn run(&mut self) -> Result<()> {
        let mut editor = DefaultEditor::new()?;
        let mut vm = Vm::new();
        let module = vm.create_root_module(ModuleSource::Internal("repl"))?;
        let palette = self.palette;
        let prompt = palette.prompt();
        let mut result_number = 1_u64;

        loop {
            // Collect garbage while waiting for input so that memory used by
            // the previous evaluation is reclaimed before the next line runs.
            let readline_result = std::thread::scope(|scope| {
                let vm = &mut vm;
                let gc = scope.spawn(move || vm.collect_garbage());
                let readline_result = editor.readline(&prompt);
                gc.join().expect("garbage collector thread panicked");
                readline_result
            });

            match readline_result {
                Ok(line) => {
                    if !line.trim().is_empty() {
                        let _ = editor.add_history_entry(line.as_str());
                        match vm.eval_str(module, &line) {
                            Ok(Val::Undefined) => {}
                            Ok(value) => {
                                let name = format!("${result_number}");
                                let symbol = vm.heap_mut().symbols.create_symbol(&name);
                                vm.heap_mut()
                                    .modules
                                    .get_mut(module)
                                    .unwrap()
                                    .set(symbol, value);
                                result_number += 1;
                                println!(
                                    "{color}{name} => {val:?}{reset}",
                                    color = palette.output,
                                    val = value.display(&vm),
                                    reset = palette.reset,
                                );
                            }
                            Err(error) => eprintln!(
                                "{}",
                                error.display(
                                    &vm,
                                    VmErrorConfig {
                                        palette: palette.error(),
                                    },
                                )
                            ),
                        }
                    }
                }
                Err(ReadlineError::Interrupted | ReadlineError::Eof) => break,
                Err(error) => return Err(error.into()),
            }
        }

        Ok(())
    }
}

impl Default for Repl {
    fn default() -> Self {
        Self::new()
    }
}

/// Run a REPL with the default palette.
pub fn run() -> Result<()> {
    Repl::new().run()
}
