use rustyline::{DefaultEditor, error::ReadlineError};
use takolisp::Vm;

const PROMPT: &str = "λ ";

fn main() -> rustyline::Result<()> {
    let mut rl = DefaultEditor::new()?;
    let mut vm = Vm::new();
    loop {
        match rl.readline(PROMPT) {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                rl.add_history_entry(line.as_str())?;
                // TODO(wmedrano): Improve value and error representations.
                match vm.eval_str(&line) {
                    Ok(val) => println!("{val:?}"),
                    Err(err) => eprintln!("{err:?}"),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(err) => return Err(err),
        }
    }
    Ok(())
}
