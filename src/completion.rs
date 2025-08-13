//! Shell completion generation using clap_complete.

use anyhow::{Context, Result};
use clap::{Command, CommandFactory};
use clap_complete::{Shell as CompletionShell, generate, generate_to};
use std::{fs, io};

use crate::cli::{AppContext, Cli, CompletionsArgs, Shell};

impl From<Shell> for CompletionShell {
    fn from(shell: Shell) -> Self {
        match shell {
            Shell::Bash => CompletionShell::Bash,
            Shell::Zsh => CompletionShell::Zsh,
            Shell::Fish => CompletionShell::Fish,
            Shell::PowerShell => CompletionShell::PowerShell,
            Shell::Elvish => CompletionShell::Elvish,
        }
    }
}

pub fn run(args: CompletionsArgs, _ctx: &AppContext) -> Result<()> {
    let mut cmd: Command = Cli::command();
    let shell: CompletionShell = args.shell.into();

    if args.stdout || args.out_dir.is_none() {
        // Generate to stdout - default when no out_dir is specified
        generate(shell, &mut cmd, "roughup", &mut io::stdout());
        return Ok(());
    }

    let dir = args.out_dir.unwrap(); // Safe because we checked is_none() above

    fs::create_dir_all(&dir).context("create --out-dir")?;
    let path = generate_to(shell, &mut cmd, "roughup", &dir).context("generate completion file")?;

    eprintln!("Wrote completion to {}", path.display());
    Ok(())
}
