use anyhow::Result;
use clap::Parser;
use roughup::{
    ContextAssembler,
    cli::{AppContext, Cli, Commands},
};
use tracing::{Level, error, info, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, time::ChronoUtc, writer::MakeWriterExt},
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

// Added comment for test

#[instrument(name = "roughup_main")]
fn main() -> Result<()>
{
    let cli = Cli::parse();

    // Initialize tracing early, before any business logic
    init_tracing(&cli)?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        command = ?cli.command,
        "Starting Roughup CLI"
    );

    // Build a context once, pass everywhere
    let ctx = AppContext {
        quiet: cli.quiet,
        no_color: cli.no_color,
        dry_run: cli.dry_run,
    };

    let result = match cli.command
    {
        Commands::Extract(args) =>
        {
            info!("Running extract command");
            roughup::core::extract_run(&args, &ctx)
        }
        Commands::Tree(args) =>
        {
            info!("Running tree command");
            roughup::tree_run(args, &ctx)
        }
        Commands::Symbols(args) =>
        {
            info!("Running symbols command");
            roughup::symbols_run(args, &ctx)
        }
        Commands::Chunk(args) =>
        {
            info!("Running chunk command");
            roughup::chunk_run(args, &ctx)
        }
        Commands::Apply(args) =>
        {
            info!("Running apply command");
            roughup::core::edit::apply_run(args, &ctx)
        }
        Commands::Preview(args) =>
        {
            info!("Running preview command");
            roughup::core::edit::preview_run(args, &ctx)
        }
        Commands::CheckSyntax(args) =>
        {
            info!("Running check-syntax command");
            roughup::core::edit::check_syntax_run(args, &ctx)
        }
        Commands::Backup(args) =>
        {
            info!("Running backup command");
            roughup::core::edit::backup_run(args, &ctx)
        }
        Commands::Init(args) =>
        {
            info!("Running init command");
            roughup::infra::config::init(args, &ctx)
        }
        Commands::Completions(args) =>
        {
            info!("Running completions command");
            roughup::completion::run(args, &ctx)
        }
        Commands::Context(args) =>
        {
            info!("Running context command");
            ContextAssembler::run(args, &ctx)
        }
        Commands::Resolve(args) =>
        {
            info!("Running resolve command");
            roughup::core::resolve_run(args, &ctx)
        }
        Commands::Anchor(args) =>
        {
            info!("Running anchor command");
            roughup::cli_ext::anchor_cmd::run_anchor_command(&args, &ctx)
        }
    };

    match &result
    {
        Ok(_) => info!("Command completed successfully"),
        Err(e) =>
        {
            error!(error = %e, "Command failed");

            // Enhanced error reporting with ariadne/miette integration
            if let Some(diagnostic) = e.downcast_ref::<miette::Report>()
            {
                // miette handles its own formatting
                eprintln!("{:?}", diagnostic);
            }
            else
            {
                // Use ariadne for other errors if they have source context
                eprintln!("Error: {}", e);

                // Print error chain
                let mut source = e.source();
                while let Some(err) = source
                {
                    eprintln!("  Caused by: {}", err);
                    source = err.source();
                }
            }
        }
    }

    result
}

/// Initialize tracing with appropriate configuration based on CLI args and environment.
fn init_tracing(cli: &Cli) -> Result<()>
{
    // Determine log level based on verbosity and quiet flags  
    let default_level = if cli.quiet { Level::ERROR } else { Level::INFO }; // Use ERROR in quiet mode to suppress all non-critical logs

    // Build filter with defaults and environment override support
    let mut env_filter = EnvFilter::builder()
        .with_default_directive(default_level.into())
        .from_env_lossy();
    
    // Only add debug directives when not in quiet mode
    if !cli.quiet {
        env_filter = env_filter
            // Add specific module filtering for development
            .add_directive("roughup=debug".parse()?)
            .add_directive("roughup::core=trace".parse()?)
            .add_directive("roughup::infra=debug".parse()?)
            .add_directive("roughup::parsers=debug".parse()?);
    }
    
    // Always reduce noise from dependencies regardless of quiet mode
    let env_filter = env_filter
        .add_directive("grep_searcher=warn".parse()?)
        .add_directive("tree_sitter=warn".parse()?)
        .add_directive("ignore=warn".parse()?)
        .add_directive("moka=warn".parse()?);

    // Configure formatting based on output preferences
    let fmt_layer = if cli.no_color
    {
        // Plain text output for CI/piping
        fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .compact()
            .with_writer(std::io::stderr.with_max_level(tracing::Level::TRACE)) // Ensure logs go to stderr
            .boxed()
    }
    else if std::env::var("RUST_LOG").is_ok()
    {
        // Detailed output for development with RUST_LOG set
        fmt::layer()
            .with_timer(ChronoUtc::rfc_3339())
            .with_target(true)
            .with_thread_ids(true)
            .with_file(true)
            .with_line_number(true)
            .with_writer(std::io::stderr.with_max_level(tracing::Level::TRACE)) // Ensure logs go to stderr
            .boxed()
    }
    else
    {
        // Clean output for normal CLI usage
        fmt::layer()
            .without_time()
            .with_target(false)
            .compact()
            .with_writer(std::io::stderr.with_max_level(tracing::Level::TRACE)) // Ensure logs go to stderr
            .boxed()
    };

    // Initialize the global subscriber
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .try_init()
        .map_err(|e| anyhow::anyhow!("Failed to initialize tracing: {}", e))?;

    Ok(())
}
