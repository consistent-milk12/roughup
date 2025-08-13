use anyhow::Result;
use clap::Parser;
use roughup::cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Extract(args) => roughup::core::extract_run(args, cli.dry_run),
        Commands::Tree(args) => roughup::tree_run(args),
        Commands::Symbols(args) => roughup::symbols_run(args),
        Commands::Chunk(args) => roughup::chunk_run(args),
        Commands::Init(args) => roughup::infra::config::init(args),
        Commands::Completions(args) => roughup::completion::run(args),
    }
}
