use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Shared application context for global flags
#[derive(Clone, Debug)]
pub struct AppContext {
    pub quiet: bool,    // global --quiet
    pub no_color: bool, // global --no-color
    pub dry_run: bool,  // global --dry-run
}

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

    /// Apply LLM-suggested edits to source files
    Apply(ApplyArgs),

    /// Preview edit changes without applying them
    Preview(PreviewArgs),

    /// Validate edit syntax and check for conflicts
    CheckSyntax(CheckSyntaxArgs),

    /// Create backup of files before editing
    Backup(BackupArgs),

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

    /// Prefer symbol boundaries when chunking
    #[arg(long, default_value = "true")]
    #[arg(action = clap::ArgAction::Set)]
    pub by_symbols: bool,

    /// Token overlap between chunks
    #[arg(long, default_value = "128")]
    pub overlap: usize,
}

#[derive(Parser)]
pub struct ApplyArgs {
    /// Edit specification file to apply
    pub edit_file: Option<PathBuf>,

    /// Read edit specification from clipboard
    #[arg(long, conflicts_with = "edit_file")]
    pub from_clipboard: bool,

    /// Preview changes without applying them (deprecated, use default behavior)
    #[arg(long)]
    pub preview: bool,

    /// Apply changes to files (required for write operations)
    #[arg(long)]
    pub apply: bool,

    /// Git repository root (auto-detected if not specified)
    #[arg(long)]
    pub repo_root: Option<PathBuf>,

    /// Create backup files before applying changes
    #[arg(long)]
    pub backup: bool,

    /// Force apply even with conflicts
    #[arg(long)]
    pub force: bool,

    /// Show verbose output during application
    #[arg(long)]
    pub verbose: bool,

    /// Apply engine: internal (fast, clear errors), git (robust, 3-way merge), auto (fallback)
    #[arg(long, default_value = "internal")]
    pub engine: ApplyEngine,

    /// Git apply mode when using git engine
    #[arg(long, default_value = "3way")]
    pub git_mode: GitMode,

    /// Context lines for patch generation
    #[arg(long, default_value = "3")]
    pub context_lines: usize,

    /// Whitespace handling for git apply
    #[arg(long, default_value = "nowarn")]
    pub whitespace: WhitespaceMode,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ApplyEngine {
    /// Fast internal engine with clear error messages
    Internal,
    /// Git apply engine with 3-way merge capability  
    Git,
    /// Try internal first, fallback to git on conflicts
    Auto,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum GitMode {
    /// 3-way merge (resilient, may leave conflict markers)
    #[value(name = "3way")]
    ThreeWay,
    /// Apply to index (requires clean preimage)
    Index,
    /// Apply to temporary worktree
    Worktree,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum WhitespaceMode {
    /// Ignore whitespace issues
    Nowarn,
    /// Warn about whitespace issues
    Warn,
    /// Fix whitespace issues automatically
    Fix,
}

#[derive(Parser)]
pub struct PreviewArgs {
    /// Edit specification file to preview
    pub edit_file: Option<PathBuf>,

    /// Read edit specification from clipboard
    #[arg(long, conflicts_with = "edit_file")]
    pub from_clipboard: bool,

    /// Show unified diff format
    #[arg(long, default_value = "true")]
    pub show_diff: bool,

    /// Git repository root (auto-detected if not specified)
    #[arg(long)]
    pub repo_root: Option<PathBuf>,

    /// Apply engine: internal (fast, clear errors), git (robust, 3-way merge), auto (fallback)
    #[arg(long, default_value = "internal")]
    pub engine: ApplyEngine,

    /// Git apply mode when using git engine
    #[arg(long, default_value = "3way")]
    pub git_mode: GitMode,

    /// Whitespace handling for git apply
    #[arg(long, default_value = "nowarn")]
    pub whitespace: WhitespaceMode,

    /// Force apply even with conflicts
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser)]
pub struct CheckSyntaxArgs {
    /// Edit specification file to validate
    pub edit_file: PathBuf,
}

#[derive(Parser)]
pub struct BackupArgs {
    /// Files to backup
    pub files: Vec<PathBuf>,

    /// Create backup before making changes (used with other commands)
    #[arg(long)]
    pub before_changes: bool,
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
