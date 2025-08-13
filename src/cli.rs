use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "roughup")]
#[command(
    about = "A super-fast, lightweight CLI for extracting and packaging source code for LLM workflows"
)]
#[command(version, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Suppress progress bars and non-essential output
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Show what would be done without executing
    #[arg(long, global = true)]
    pub dry_run: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Extract specific line ranges from files
    Extract(ExtractArgs),

    /// Display project tree structure
    Tree(TreeArgs),

    /// Extract symbol information from source files
    Symbols(SymbolsArgs),

    /// Split extracted content into token-sized chunks
    Chunk(ChunkArgs),

    /// Initialize a roughup.toml config file
    Init(InitArgs),

    /// Generate shell completions
    Completions(CompletionsArgs),
}

#[derive(Parser)]
pub struct ExtractArgs {
    /// Files and line ranges (format: file.rs:10-20,25-30)
    pub targets: Vec<String>,

    /// Output file path
    #[arg(short, long, default_value = "extracted_source.txt")]
    pub output: PathBuf,

    /// Annotate each extraction with file and line info
    #[arg(long)]
    pub annotate: bool,

    /// Wrap extractions in fenced code blocks
    #[arg(long)]
    pub fence: bool,

    /// Copy result to clipboard
    #[arg(long)]
    pub clipboard: bool,

    /// Show what would be done without executing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser)]
pub struct TreeArgs {
    /// Root directory to scan
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Additional glob patterns to ignore
    #[arg(short, long)]
    pub ignore: Vec<String>,

    /// Maximum depth to traverse
    #[arg(short, long)]
    pub depth: Option<usize>,

    /// Show what would be done without executing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Parser)]
pub struct SymbolsArgs {
    /// Root directory to scan
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Languages to include (rust, python, javascript, typescript, go, cpp)
    #[arg(short, long)]
    pub languages: Vec<String>,

    /// Output file path
    #[arg(short, long, default_value = "symbols.jsonl")]
    pub output: PathBuf,

    /// Include private symbols
    #[arg(long)]
    pub include_private: bool,
}

#[derive(Parser)]
pub struct ChunkArgs {
    /// Input file to chunk
    pub input: PathBuf,

    /// Maximum tokens per chunk
    #[arg(long, default_value = "4000")]
    pub max_tokens: usize,

    /// GPT model (gpt-4, gpt-4o, gpt-3.5-turbo) or encoding (o200k_base, cl100k_base)
    #[arg(short, long, default_value = "gpt-4o")]
    pub model: String,

    /// Output directory for chunks
    #[arg(short, long, default_value = "chunks")]
    pub output_dir: PathBuf,

    /// Prefer symbol boundaries when chunking (default: true)
    #[arg(long, default_value = "true")]
    pub by_symbols: bool,

    /// Token overlap between chunks
    #[arg(long, default_value = "128")]
    pub overlap: usize,
}

#[derive(Parser)]
pub struct InitArgs {
    /// Directory to initialize config in
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Overwrite existing config file
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Elvish,
}

#[derive(Parser)]
pub struct CompletionsArgs {
    /// Target shell
    #[arg(value_enum)]
    pub shell: Shell,

    /// Output directory; if omitted and --stdout not set, prints error
    #[arg(long)]
    pub out_dir: Option<PathBuf>,

    /// Print completion script to stdout instead of a file
    #[arg(long)]
    pub stdout: bool,
}
