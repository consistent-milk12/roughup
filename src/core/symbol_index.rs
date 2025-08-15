use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::symbols::{Symbol, SymbolKind};

/// Options for symbol lookup and ranking
#[derive(Debug, Clone, Default)]
pub struct LookupOptions<'a>
{
    /// Prefer fuzzy searching when true; otherwise exact/prefix/substring
    pub semantic: bool,

    /// Anchor file used for scope & proximity boosts
    pub anchor_file: Option<&'a Path>,

    /// Optional anchor line (1-based)
    pub anchor_line: Option<usize>,

    /// Previously-selected qualified names to boost
    pub history: Option<&'a HashSet<String>>,

    /// Maximum results to return
    pub limit: usize,

    /// Optional kind filters
    pub kinds: Option<&'a [SymbolKind]>,
}

impl<'a> LookupOptions<'a>
{
    pub fn with_limit(
        mut self,
        limit: usize,
    ) -> Self
    {
        self.limit = limit;
        self
    }
}

/// In-memory index over `symbols.jsonl`
pub struct SymbolIndex
{
    /// All symbols loaded from the index, sorted deterministically
    symbols: Vec<Symbol>,

    /// Maps simple symbol names (lowercase) to their indices in `symbols`
    name_to_idxs: HashMap<String, Vec<usize>>,

    /// Maps file paths to indices of symbols in those files (sorted by start line)
    file_to_idxs: BTreeMap<PathBuf, Vec<usize>>,

    /// Regex used for tokenizing symbol names (snake/camel case)
    snake_re: Regex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedSymbol
{
    /// The symbol matched in the lookup
    pub symbol: Symbol,

    /// (semantic, scope, proximity, history) — for deterministic lexicographic ordering
    pub score: (u8, u8, u8, u8),
}

impl SymbolIndex
{
    /// Loads symbols from a JSONL file and builds the index.
    ///
    /// - Reads each line as a JSON-encoded `Symbol`.
    /// - Skips empty lines.
    /// - Sorts symbols deterministically by file path and start line.
    /// - Builds lookup maps for fast queries.
    pub fn load(jsonl: &Path) -> Result<Self>
    {
        // Open the symbols file
        let f = File::open(jsonl)
            .with_context(|| format!("Failed to open symbols file: {}", jsonl.display()))?;
        let reader = BufReader::new(f);
        let mut symbols: Vec<Symbol> = Vec::new();

        // Parse each line as a Symbol
        for (i, line) in reader
            .lines()
            .enumerate()
        {
            let line = line.with_context(|| format!("Failed to read line {}", i + 1))?;
            if line
                .trim()
                .is_empty()
            {
                continue;
            }
            let s: Symbol = serde_json::from_str(&line)
                .with_context(|| format!("Failed to parse JSON on line {}", i + 1))?;
            symbols.push(s);
        }

        // Sort symbols by file path, then start_line, then end_line for deterministic order
        symbols.sort_by(|a, b| {
            (
                a.file
                    .clone(),
                a.start_line,
                a.end_line,
            )
                .cmp(&(
                    b.file
                        .clone(),
                    b.start_line,
                    b.end_line,
                ))
        });

        // Build name-to-indices and file-to-indices maps
        let mut name_to_idxs: HashMap<String, Vec<usize>> = HashMap::new();
        let mut file_to_idxs: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (idx, s) in symbols
            .iter()
            .enumerate()
        {
            name_to_idxs
                .entry(
                    s.name
                        .to_ascii_lowercase(),
                )
                .or_default()
                .push(idx);
            file_to_idxs
                .entry(
                    s.file
                        .clone(),
                )
                .or_default()
                .push(idx);
        }

        // Regex for tokenizing symbol names (snake/camel case)
        Ok(Self {
            symbols,
            name_to_idxs,
            file_to_idxs,
            snake_re: Regex::new(r"[A-Za-z0-9]+").unwrap(),
        })
    }

    pub fn all(&self) -> &[Symbol]
    {
        &self.symbols
    }

    /// Lookup by query string and options. Returns ranked matches.
    pub fn lookup<'a>(
        &'a self,
        query: &str,
        opts: LookupOptions<'a>,
    ) -> Vec<RankedSymbol>
    {
        let q = query.trim();
        if q.is_empty()
        {
            return Vec::new();
        }
        let ql = q.to_ascii_lowercase();

        // Pre-collect candidate indices using quick filters to keep it fast.
        let mut candidates: Vec<usize> = Vec::new();

        // 1) Fast path: exact simple-name match
        if let Some(ix) = self
            .name_to_idxs
            .get(&ql)
        {
            candidates.extend(
                ix.iter()
                    .copied(),
            );
        }

        // 2) Substring/prefix in simple or qualified names
        let ql_clone = ql.clone();
        let more: Vec<usize> = self
            .symbols
            .par_iter()
            .enumerate()
            .filter(|(_, s)| {
                let name = s
                    .name
                    .to_ascii_lowercase();
                let qn = s
                    .qualified_name
                    .to_ascii_lowercase();
                // Keep old substring behavior for candidate collection
                // Semantic scoring will filter out noise later
                name.starts_with(&ql_clone)
                    || name.contains(&ql_clone)
                    || qn.ends_with(&ql_clone)
                    || qn.contains(&ql_clone)
            })
            .map(|(i, _)| i)
            .collect();
        candidates.extend(more);

        // 3) Include all symbols from anchor directory for scope/proximity scoring
        if let Some(anchor_dir) = opts
            .anchor_file
            .and_then(|p| p.parent())
        {
            let anchor_more: Vec<usize> = self
                .symbols
                .par_iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.file
                        .starts_with(anchor_dir)
                })
                .map(|(i, _)| i)
                .collect();
            candidates.extend(anchor_more);
        }

        // 3) If semantic, include fuzzy token matches
        if opts.semantic
        {
            let tokens = self.tokens(&ql);
            let sem_more: Vec<usize> = self
                .symbols
                .par_iter()
                .enumerate()
                .filter(|(_, s)| {
                    self.token_hit(&tokens, &s.name) || self.token_hit(&tokens, &s.qualified_name)
                })
                .map(|(i, _)| i)
                .collect();
            candidates.extend(sem_more);
        }

        // Dedup & stable sort
        candidates.sort();
        candidates.dedup();

        // Optional kind filter
        if let Some(kinds) = opts.kinds
        {
            let set: HashSet<SymbolKind> = kinds
                .iter()
                .cloned()
                .collect();
            candidates.retain(|&i| set.contains(&self.symbols[i].kind));
        }

        // Compute scores and rank
        let anchor_dir = opts
            .anchor_file
            .and_then(|p| {
                p.parent()
                    .map(|x| x.to_path_buf())
            });
        let anchor_file = opts
            .anchor_file
            .map(|p| p.to_path_buf());
        let anchor_line = opts.anchor_line;
        let history = opts.history;

        let mut ranked: Vec<RankedSymbol> = candidates
            .into_iter()
            .map(|i| {
                let s = &self.symbols[i];
                let semantic = self.semantic_score(&ql, s);
                let scope = self.scope_score(anchor_dir.as_ref(), &s.file);
                let proximity = self.proximity_score(anchor_file.as_ref(), anchor_line, s);
                let hist = if let Some(h) = history
                {
                    if h.contains(&s.qualified_name) { 1 } else { 0 }
                }
                else
                {
                    0
                };
                RankedSymbol {
                    symbol: s.clone(),
                    score: (semantic, scope, proximity, hist),
                }
            })
            .collect();

        ranked.sort_by_key(|it| {
            (
                std::cmp::Reverse(
                    it.score
                        .0,
                ), // semantic: higher first
                std::cmp::Reverse(
                    it.score
                        .1,
                ), // scope: higher first
                std::cmp::Reverse(
                    it.score
                        .2,
                ), // proximity: higher first
                std::cmp::Reverse(
                    it.score
                        .3,
                ), // history: higher first
                it.symbol
                    .file
                    .clone(), // tiebreak: path asc
                it.symbol
                    .start_line, // tiebreak: line asc
                it.symbol
                    .qualified_name
                    .clone(), // tiebreak: name asc
            )
        });

        let limit = opts
            .limit
            .max(1);
        ranked.truncate(limit);
        ranked
    }

    fn tokens(
        &self,
        s: &str,
    ) -> Vec<String>
    {
        self.snake_re
            .find_iter(s)
            .map(|m| {
                m.as_str()
                    .to_ascii_lowercase()
            })
            .collect()
    }

    fn token_hit(
        &self,
        tokens: &[String],
        name: &str,
    ) -> bool
    {
        let nl = name.to_ascii_lowercase();
        tokens
            .iter()
            .all(|t| nl.contains(t))
    }

    /// Compute a conservative semantic score for a symbol name.
    /// 3 = exact (case-insensitive)
    /// 2 = prefix (len >= 2)
    /// 1 = token/segment match across non-alnum splits (len >= 2)
    /// 0 = otherwise
    fn semantic_score(
        &self,
        query: &str,
        s: &Symbol,
    ) -> u8
    {
        // Normalize both to lowercase for case-insensitive compare
        let q = query
            .trim()
            .to_lowercase(); // normalized query
        let n = s
            .name
            .trim()
            .to_lowercase(); // normalized name
        // If query is empty, no semantic lift
        if q.is_empty()
        {
            // guard for empty
            return 0; // no score
        }
        // Exact match → strongest signal
        if n == q
        {
            // exact match
            return 3; // score 3
        }
        // Prefix match (require length >= 2 to avoid noise)
        if q.len() >= 2 && n.starts_with(&q)
        {
            // prefix match
            return 2; // score 2
        }
        // Segment/token contains (split by non-alnum), also require len >= 2
        if q.len() >= 2                                   // avoid single-char noise
            && n.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|seg| !seg.is_empty() && seg == q)
        // token equals query
        {
            return 1; // score 1
        }
        // Otherwise no semantic similarity credit
        0 // score 0
    }

    /// Computes scope score based on anchor directory and symbol file.
    ///
    /// - Returns 1 if the symbol's file is within the anchor directory.
    /// - Returns 0 otherwise.
    fn scope_score(
        &self,
        anchor_dir: Option<&PathBuf>,
        file: &Path,
    ) -> u8
    {
        if let Some(dir) = anchor_dir
        {
            // Use string-based comparison for relative paths to avoid canonicalize issues
            let dir_str = dir.to_string_lossy();
            let file_str = file.to_string_lossy();

            // Check if file path starts with directory path
            if file_str.starts_with(dir_str.as_ref())
            {
                return 1;
            }
        }

        0
    }

    /// Computes proximity score based on anchor file and line.
    ///
    /// - Returns 3 if symbol is in anchor file (regardless of line distance)
    /// - Returns 2 if symbol is within 20 lines of anchor line in same file
    /// - Returns 1 if within 100 lines of anchor line in same file
    /// - Returns 0 otherwise
    fn proximity_score(
        &self,
        anchor_file: Option<&PathBuf>,
        anchor_line: Option<usize>,
        s: &Symbol,
    ) -> u8
    {
        if let Some(af) = anchor_file
            && &s.file == af
        {
            if let Some(al) = anchor_line
            {
                // Close (≤20 lines): 2, medium (≤100): 1, else 0 for line distance
                let d = if al < s.start_line
                {
                    s.start_line - al
                }
                else
                {
                    al.saturating_sub(s.end_line)
                };

                if d <= 20
                {
                    2
                }
                else if d <= 100
                {
                    1
                }
                else
                {
                    3 // Same file but far from line gets base score
                }
            }
            else
            {
                3 // Same file, no line anchor
            }
        }
        else
        {
            0
        }
    }

    /// Get all symbols in a file (deterministic order)
    pub fn symbols_in_file(
        &self,
        file: &Path,
    ) -> &[usize]
    {
        self.file_to_idxs
            .get(file)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}
