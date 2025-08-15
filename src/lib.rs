//! **roughup** - Super-fast Rust CLI for extracting/packaging source code for LLM workflows (test)
//!
//! Smart gitignore-aware processing with parallel execution and tree-sitter symbol extraction.
//! Performance-first design with memory-mapped I/O and AST caching.

/// Command-line interface with clap integration
pub mod cli;

/// Shell completion generation
pub mod completion;

/// Core processing pipeline - High-performance extraction and analysis (2,847 lines total)
pub mod core {
    /// Line-range extraction with gitignore awareness and memory mapping
    pub mod extract;
    pub use extract::run as extract_run;

    /// Edit format parsing and application system for LLM collaboration
    pub mod edit;
    pub use edit::{EditConflict, EditEngine, EditOperation, EditResult, EditSpec};

    /// Centralized backup system with session-scoped storage
    pub mod backup;
    pub use backup::{BackupManager, SessionManifest, list_sessions, read_session_manifest};

    /// Backup session management operations (list, show, restore, cleanup)
    pub mod backup_ops;
    pub use backup_ops::{ListRequest, SessionInfo, ShowRequest, ShowResponse};

    /// EBNF to unified diff patch converter for Git integration
    pub mod patch;
    pub use patch::{
        FilePatch, Hunk, PatchConfig, PatchSet, generate_patches, render_unified_diff,
    };

    /// Git apply integration with 3-way merge and error mapping
    pub mod git;
    pub use git::{GitConflict, GitEngine, GitMode, GitOptions, GitOutcome, Whitespace};

    /// Unified apply engine trait for hybrid architecture
    pub mod apply_engine;
    pub use apply_engine::{ApplyEngine, ApplyReport, Engine, Preview, create_engine};

    /// Git conflict marker detection and resolution (Phase 3.5)
    pub mod conflict;
    pub use conflict::{ConflictMarker, ConflictOrigin, ConflictType, parse_conflicts, score_conflict};

    /// Conflict resolution strategies with SmartMerge pipeline
    pub mod resolve;
    pub use resolve::{Resolution, ResolveStrategy, resolve, resolve_no_check, resolve_batch, run as resolve_run};

    /// Tree-sitter symbol extraction pipeline (Rust+Python locked, 572 lines)
    pub mod symbols;
    pub use symbols::{Symbol, SymbolKind, Visibility, run as symbols_run};

    /// Directory tree visualization with depth control and parallel processing
    pub mod tree;
    pub use tree::run as tree_run;

    /// Token-aware content chunking for LLM workflows with tiktoken integration
    pub mod chunk;
    pub use chunk::run as chunk_run;

    pub mod budgeter;
    pub mod context;
    /// Smart context assembly (Phase 3)
    pub mod symbol_index;
    pub use context::run as context_run;
}

/// Language processing - AST parsing and symbol extraction with moka caching
pub mod parsers {
    /// Rust symbol extraction with tree-sitter (qualified names, visibility)
    pub mod rust_parser;
    pub use rust_parser::RustExtractor;

    /// Python symbol extraction with tree-sitter (classes, functions, methods)
    pub mod python_parser;
    pub use python_parser::PythonExtractor;

    // Re-export common extractor interface
    pub use crate::core::symbols::{SymbolExtractor, get_extractor};
}

/// Infrastructure - Configuration, I/O, and utilities (lean architecture)
pub mod infra {
    /// Configuration management with TOML support and feature flags
    pub mod config;
    pub use config::{Config, init as config_init, load_config};

    /// Memory-mapped file I/O for large files (>1MB threshold)
    pub mod io;
    pub use io::{FileContent, read_file_smart};

    /// CRLF/LF-robust line indexing for O(1) line→byte mapping
    pub mod line_index;
    pub use line_index::NewlineIndex;

    /// Gitignore-aware directory walking with rayon parallelism
    pub mod walk;
    pub use walk::FileWalker;

    /// Utility functions and helpers for common operations
    pub mod utils;
    // Keep utils private - not part of the public API
}

// Strategic re-exports for clean CLI interface
pub use cli::{AppContext, Cli, Commands};
pub use core::{
    chunk_run, context_run, extract_run, generate_patches, render_unified_diff, symbols_run,
    tree_run,
};
pub use infra::{Config, FileWalker, load_config};
pub use parsers::{PythonExtractor, RustExtractor, SymbolExtractor};
// Phase 3 re-exports
pub use core::context;

// Core types for external consumers
pub use core::symbols::{Symbol, SymbolKind, Visibility};
