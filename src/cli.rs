use clap::{Args, Parser, Subcommand, ValueEnum};
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

    /// Assemble a smart, token-budgeted context for LLMs
    Context(ContextArgs),
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

    /// Add N lines of context around each range (before and after)
    #[arg(long, default_value = "0")]
    pub context: usize,

    /// Merge ranges that are at most N lines apart
    #[arg(long, default_value = "0")]
    pub merge_within: usize,

    /// Use this GPT model for token estimation (e.g., gpt-4o, o200k_base)
    #[arg(long, default_value = "gpt-4o")]
    pub model: String,

    /// Token budget for the final assembled context
    #[arg(long)]
    pub budget: Option<usize>,

    /// Remove common leading indentation within each snippet
    #[arg(long)]
    pub dedent: bool,

    /// Squeeze blank lines in the output
    #[arg(long, default_value = "false")]
    pub squeeze_blank: bool,
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

    /// Output results in JSON format (single line)
    #[arg(long)]
    pub json: bool,
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
    /// Apply to temporary worktree (experimental, currently disabled)
    #[value(help = "Worktree mode (not implemented)")]
    Worktree,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
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

    /// Context lines for patch generation (matches --apply)
    #[arg(long, default_value = "3")]
    pub context_lines: usize,
}

#[derive(Parser)]
pub struct CheckSyntaxArgs {
    /// Edit specification file to validate
    pub edit_file: PathBuf,
}

#[derive(Parser)]
pub struct BackupArgs {
    #[command(subcommand)]
    pub command: BackupSubcommand,
}

#[derive(Subcommand)]
pub enum BackupSubcommand {
    /// List backup sessions with optional filtering
    List(BackupListArgs),

    /// Show detailed information about a backup session
    Show(BackupShowArgs),

    /// Restore files from a backup session
    Restore(BackupRestoreArgs),

    /// Clean up old backup sessions
    Cleanup(BackupCleanupArgs),
}

#[derive(Parser, Debug)]
pub struct BackupListArgs {
    /// Filter: only successful sessions
    #[arg(long)]
    pub successful: bool,

    /// Filter by engine (internal, git, auto)
    #[arg(long)]
    pub engine: Option<String>,

    /// Filter by relative time (e.g., "7d", "24h")
    #[arg(long, value_name = "SPAN")]
    pub since: Option<String>,

    /// Limit result count
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Sort order (desc or asc)
    #[arg(long, default_value = "desc")]
    pub sort: String,

    /// Machine-readable JSON output
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct BackupShowArgs {
    /// Session identifier (full or short)
    pub id: String,

    /// Include file-level details
    #[arg(long)]
    pub verbose: bool,

    /// JSON output
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct BackupRestoreArgs {
    /// Session ID or alias (e.g., 'latest')
    pub session: String,

    /// Restore only this repo-relative path from the session
    #[arg(long)]
    pub path: Option<PathBuf>,

    /// Do not write files; show plan and (optional) diff
    #[arg(long)]
    pub dry_run: bool,

    /// Overwrite even if current content differs
    #[arg(long)]
    pub force: bool,

    /// Show unified diff for single-file restores
    #[arg(long)]
    pub show_diff: bool,

    /// Validate backed-up content against manifest checksums
    #[arg(long)]
    pub verify_checksum: bool,

    /// Back up current files before overwriting
    #[arg(long)]
    pub backup_current: bool,

    /// Emit JSON result instead of human text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct BackupCleanupArgs {
    /// RFC3339 or relative span: 7d, 24h, 90m, 45s, 2w
    #[arg(long)]
    pub older_than: Option<String>,

    /// Keep N newest sessions; remove the rest
    #[arg(long)]
    pub keep_latest: Option<usize>,

    /// Include sessions without DONE marker
    #[arg(long)]
    pub include_incomplete: bool,

    /// Simulate without deleting anything
    #[arg(long)]
    pub dry_run: bool,

    /// Emit JSON result instead of human text
    #[arg(long)]
    pub json: bool,
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

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum ContextTemplate {
    Refactor,
    Bugfix,
    Feature,
    Freeform,
}

#[derive(Parser, Debug)]
pub struct ContextArgs {
    /// Query strings (symbol names or qualified names)
    #[arg(value_name = "QUERY", required = true)]
    pub queries: Vec<String>,

    /// Project root (used for relative paths)
    #[arg(long, default_value = ".")]
    pub path: std::path::PathBuf,

    /// Symbols index file (JSONL) produced by `rup symbols`
    #[arg(long, default_value = "symbols.jsonl")]
    pub symbols: std::path::PathBuf,

    /// GPT model or encoding for token estimation (e.g., gpt-4o, o200k_base)
    #[arg(long, default_value = "gpt-4o")]
    pub model: Option<String>,

    /// Token budget for the final assembled context
    #[arg(long, default_value = "6000")]
    pub budget: Option<usize>,

    /// Use fuzzy/semantic matching in addition to exact/substring
    #[arg(long)]
    pub semantic: bool,

    /// Preferred template
    #[arg(long, value_enum, default_value_t = ContextTemplate::Freeform)]
    pub template: ContextTemplate,

    /// Anchor file to prefer for local scope/proximity ranking
    #[arg(long)]
    pub anchor: Option<std::path::PathBuf>,

    /// Anchor line number (1-based)
    #[arg(long)]
    pub anchor_line: Option<usize>,

    /// Limit per-query candidates before merging
    #[arg(long, default_value_t = 8)]
    pub top_per_query: usize,

    /// Overall maximum candidates to include before budget
    #[arg(long, default_value_t = 256)]
    pub limit: usize,

    /// Wrap excerpts in fenced code blocks
    #[arg(long)]
    pub fence: bool,

    /// Emit JSON output (single-line)
    #[arg(long)]
    pub json: bool,

    /// Copy result to clipboard
    #[arg(long)]
    pub clipboard: bool,
}
