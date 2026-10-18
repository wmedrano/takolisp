use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use takolisp::{
    ModuleSource, Vm,
    error::VmErrorConfig,
    repl::{Palette, Repl},
};

#[derive(Parser)]
#[command(name = "takolisp", about = "The Takolisp interpreter")]
struct Cli {
    /// Force colored output.
    #[arg(long)]
    color: bool,

    /// Evaluate Lisp files in order. With no files, start the interactive REPL.
    #[arg(value_name = "FILE")]
    files: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let palette = if cli.color {
        Palette::COLOR
    } else {
        Palette::PLAIN
    };

    if cli.files.is_empty() {
        Repl::new().with_palette(palette).run()
    } else {
        run_script(&cli.files, palette)
    }
}

fn run_script(files: &[PathBuf], palette: Palette) -> Result<()> {
    let mut vm = Vm::new();
    let source = files
        .first()
        .cloned()
        .map_or_else(|| ModuleSource::Internal("script"), ModuleSource::File);
    let module = vm.create_root_module(source)?;

    for file in files {
        let source = std::fs::read_to_string(file)
            .with_context(|| format!("failed to read {}", file.display()))?;
        vm.eval_str(module, &source).map_err(|error| {
            anyhow!(
                "{}",
                error.display(
                    &vm,
                    VmErrorConfig {
                        palette: palette.error(),
                    },
                )
            )
        })?;
    }

    Ok(())
}
