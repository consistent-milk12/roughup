use anyhow::Result;
use clap::Parser;
use roughup::cli::{AppContext, Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Build a context once, pass everywhere
    let ctx = AppContext {
        quiet: cli.quiet,
        no_color: cli.no_color,
        dry_run: cli.dry_run,
    };

    match cli.command {
        Commands::Extract(args) => roughup::core::extract_run(args, &ctx),
        Commands::Tree(args) => roughup::tree_run(args, &ctx),
        Commands::Symbols(args) => roughup::symbols_run(args, &ctx),
        Commands::Chunk(args) => roughup::chunk_run(args, &ctx),
        Commands::Init(args) => roughup::infra::config::init(args, &ctx),
        Commands::Completions(args) => roughup::completion::run(args, &ctx),
    }
}
