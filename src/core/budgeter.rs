use anyhow::{Context, Result, anyhow};
use moka::sync::Cache;
use std::collections::HashMap;
use tiktoken_rs::{CoreBPE, cl100k_base, get_bpe_from_model, o200k_base};
use xxhash_rust::xxh64::Xxh64;

/// Enhanced priority system with fine-grained scoring
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Priority {
    /// Primary priority level (0-255, higher = more important)
    pub level: u8,
    /// Secondary relevance score (0.0-1.0, higher = more relevant)
    pub relevance: f32,
    /// Tertiary proximity score (0.0-1.0, higher = closer to anchor)
    pub proximity: f32,
}

impl Priority {
    /// Create a high priority with perfect relevance and proximity
    pub const fn high() -> Self {
        Self {
            level: 200,
            relevance: 1.0,
            proximity: 1.0,
        }
    }

    /// Create a medium priority with good relevance
    pub const fn medium() -> Self {
        Self {
            level: 100,
            relevance: 0.7,
            proximity: 0.5,
        }
    }

    /// Create a low priority with minimal relevance
    pub const fn low() -> Self {
        Self {
            level: 50,
            relevance: 0.3,
            proximity: 0.1,
        }
    }

    /// Create a custom priority with all dimensions, NaN-safe
    pub fn custom(level: u8, relevance: f32, proximity: f32) -> Self {
        fn sane(x: f32) -> f32 {
            if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
        }
        Self {
            level,
            relevance: sane(relevance),
            proximity: sane(proximity),
        }
    }

    /// Calculate final composite score for ranking
    pub fn composite_score(&self) -> f64 {
        // Weighted combination: level is primary, relevance secondary, proximity tertiary
        (self.level as f64) * 1000.0 + (self.relevance as f64) * 100.0 + (self.proximity as f64) * 10.0
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for Priority {}

// Natural ascending order - descending handled at call sites
impl std::cmp::Ord for Priority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.level
            .cmp(&other.level)
            .then_with(|| self.relevance.total_cmp(&other.relevance))
            .then_with(|| self.proximity.total_cmp(&other.proximity))
    }
}

// Maintain backward compatibility with the old enum values
impl From<Priority> for u8 {
    fn from(priority: Priority) -> Self {
        match priority.level {
            200..=255 => 2, // High
            100..=199 => 1, // Medium
            _ => 0,         // Low
        }
    }
}

/// A context item candidate to budget
#[derive(Debug, Clone)]
pub struct Item {
    /// Unique identifier for the item (e.g., file:line-span or qualified name)
    pub id: String,

    /// Body text of the item
    pub content: String,

    /// Selection priority for budgeting
    pub priority: Priority,

    /// Whether this item must be included (trim if necessary)
    pub hard: bool,

    /// Minimal tokens we should try to keep for this item
    pub min_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct FitResult {
    /// Items that fit within the budget
    pub items: Vec<FittedItem>,

    /// Total number of tokens used by fitted items
    pub total_tokens: usize,
}

#[derive(Debug, Clone)]
pub struct FittedItem {
    /// The trimmed or fitted content for this item
    pub content: String,

    /// Unique identifier for the item
    pub id: String,

    /// Number of tokens used by this fitted content
    pub tokens: usize,
}

/// Budget manager backed by tiktoken-rs with token caching
pub struct Budgeter {
    /// Byte Pair Encoding (BPE) tokenizer for counting tokens
    bpe: CoreBPE,

    /// Token count cache for fast repeated queries
    cache: Cache<u64, usize>,
}

impl Budgeter {
    /// Create a new Budgeter for a given model or encoding name.
    ///
    /// Supported values include model names (e.g., "gpt-3.5-turbo", "gpt-4") or encoding names
    /// ("cl100k_base", "o200k_base"). Falls back to encoding names if model lookup fails.
    ///
    /// # Arguments
    /// * `model_or_encoding` - Model or encoding name (case-insensitive).
    ///
    /// # Errors
    /// Returns an error if the model or encoding is unsupported or cannot be loaded.
    pub fn new(model_or_encoding: &str) -> Result<Self> {
        let lower = model_or_encoding.to_ascii_lowercase();

        // Try to get BPE from model name first, fallback to encoding name.
        let bpe = match get_bpe_from_model(&lower) {
            Ok(b) => b,
            Err(_) => match lower.as_str() {
                "o200k_base" => o200k_base().context("load o200k_base")?,
                "cl100k_base" => cl100k_base().context("load cl100k_base")?,
                _ => return Err(anyhow!("Unsupported model/encoding: {model_or_encoding}")),
            },
        };

        // Create Budgeter with a token count cache of 100,000 entries.
        Ok(Self {
            bpe,
            cache: Cache::new(100_000),
        })
    }

    /// Count the number of tokens in the given string, using cache for efficiency.
    /// Uses xxhash64 to hash the string as cache key.
    pub fn count(&self, s: &str) -> usize {
        // Create a new xxhash64 hasher with seed 0
        let mut hasher = Xxh64::new(0);

        // Feed the string bytes into the hasher
        hasher.update(s.as_bytes());

        // Get the hash digest as cache key
        let key = hasher.digest();

        // Check if the token count is already cached
        if let Some(t) = self.cache.get(&key) {
            return t;
        }

        // Otherwise, encode and count tokens
        let t = self.bpe.encode_ordinary(s).len();

        // Insert the result into cache
        self.cache.insert(key, t);

        t
    }

    /// Fit items into `budget_tokens` deterministically with trimming.
    /// Fit items into `budget_tokens` deterministically with trimming.
    ///
    /// - Items are sorted by priority (descending) and id (ascending) for deterministic selection.
    /// - "Hard" items (must be included) are reserved first with their minimal token requirement.
    /// - Non-hard items are added fully if they fit, otherwise trimmed if possible.
    /// - If minimal hard items couldn't be placed, attempts to trim existing hard items.
    /// - As a last resort, trims from the lowest priority tail to fit within the budget.
    pub fn fit(&self, mut items: Vec<Item>, budget_tokens: usize) -> Result<FitResult> {
        // Deterministic order: by (priority desc, id asc)
        items.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));

        let mut out: Vec<FittedItem> = Vec::new();
        let mut remaining = budget_tokens;

        // 1) Reserve hard items minimally
        let hard_first: Vec<Item> = items.iter().filter(|&i| i.hard).cloned().collect();

        for it in &hard_first {
            let need = it.min_tokens.max(1);

            if remaining < need {
                // Not enough budget for this hard item; will try to trim later
                continue;
            }

            let (s, tok) = self.take_prefix(&it.content, need);

            out.push(FittedItem {
                id: it.id.clone(),
                content: s,
                tokens: tok,
            });

            remaining = remaining.saturating_sub(tok);
        }

        // 2) Add non-hard items fully while they fit
        for it in items.into_iter() {
            if hard_first.iter().any(|h| h.id == it.id) {
                continue;
            }
            let tok = self.count(&it.content);
            if tok <= remaining {
                // Whole item fits in the remaining budget
                out.push(FittedItem {
                    id: it.id,
                    content: it.content,
                    tokens: tok,
                });
                remaining -= tok;
            } else if it.min_tokens > 0 && remaining >= it.min_tokens {
                // Item doesn't fit fully, but can be trimmed to min_tokens
                let (s, t) = self.take_prefix(&it.content, remaining);
                out.push(FittedItem {
                    id: it.id,
                    content: s,
                    tokens: t,
                });
                remaining = 0;
                break;
            }
        }

        // 3) If we couldn't place minimal hard pieces earlier, attempt to trim existing hard items
        if remaining == 0 {
            // Ensure total <= budget (guard against rounding)
            let tot: usize = out.iter().map(|x| x.tokens).sum();
            if tot <= budget_tokens {
                return Ok(FitResult {
                    items: out,
                    total_tokens: tot,
                });
            }
        }

        // After step 1, expand hard items toward full content in priority order
        for it in &hard_first {
            if remaining == 0 {
                break;
            }
            // Find the fitted entry we already pushed for this id
            if let Some(fi) = out.iter_mut().find(|fi| fi.id == it.id) {
                let full_tok = self.count(&it.content);
                if fi.tokens < full_tok {
                    let want = (fi.tokens + remaining).min(full_tok);
                    let (s, t) = self.take_prefix(&it.content, want);
                    // Increase only if we gained tokens
                    if t > fi.tokens {
                        remaining -= t - fi.tokens;
                        fi.tokens = t;
                        fi.content = s;
                    }
                }
            }
        }

        // After assembling `out`, ensure we do not exceed the budget
        let mut total_tokens: usize = out.iter().map(|x| x.tokens).sum();
        if total_tokens > budget_tokens {
            // Map of id -> (is_hard, min_tokens)
            let mut meta: HashMap<&str, (bool, usize)> = HashMap::new();
            for h in &hard_first {
                meta.insert(h.id.as_str(), (true, h.min_tokens.max(1)));
            }

            // 1) Drop non-hard items from the end until we're under budget or none left
            let mut idx = out.len();
            while total_tokens > budget_tokens && idx > 0 {
                let k = idx - 1;
                let id = out[k].id.as_str();
                let is_hard = meta.get(id).map(|m| m.0).unwrap_or(false);
                if !is_hard {
                    total_tokens = total_tokens.saturating_sub(out[k].tokens);
                    out.remove(k);
                }
                idx = k;
            }

            // 2) If still over budget, trim hard items (never below min_tokens)
            if total_tokens > budget_tokens {
                // Build a vector of (index, current_tokens, min_tokens)
                let mut hard_indices: Vec<(usize, usize, usize)> = out
                    .iter()
                    .enumerate()
                    .filter_map(|(j, fi)| {
                        meta.get(fi.id.as_str()).map(|(_, min)| (j, fi.tokens, *min))
                    })
                    .collect();

                // Trim from lowest-priority tail first (out is already in priority order)
                hard_indices.reverse();

                let mut excess = total_tokens - budget_tokens;
                for (j, cur, min_needed) in hard_indices {
                    if excess == 0 {
                        break;
                    }
                    if cur > min_needed {
                        let reducible = cur - min_needed;
                        let cut = reducible.min(excess);

                        // Re-trim this item to (cur - cut) tokens
                        let want = cur - cut;
                        let (new_text, new_tok) = self.take_prefix(&out[j].content, want);
                        out[j].content = new_text;
                        out[j].tokens = new_tok;

                        excess -= cut;
                        total_tokens -= cut;
                    }
                }

                // If still over budget after all trims, drop the smallest tail item
                if total_tokens > budget_tokens && !out.is_empty() {
                    // Remove the last item; this keeps determinism straightforward
                    let last = out.pop().unwrap();
                    total_tokens = total_tokens.saturating_sub(last.tokens);
                }
            }
        }

        let total_tokens = total_tokens;

        Ok(FitResult {
            items: out,
            total_tokens,
        })
    }

    /// Return a prefix with at most `max_tokens` tokens, with a clean ellipsis boundary
    /// Returns a prefix of `s` containing at most `max_tokens` tokens.
    /// If the content is trimmed, appends an ellipsis ("…\n") for clarity.
    /// Ensures the result ends with a newline.
    fn take_prefix(&self, s: &str, max_tokens: usize) -> (String, usize) {
        // If no tokens are allowed, return empty string and zero tokens.
        if max_tokens == 0 {
            return (String::new(), 0);
        }

        // Encode the string into token IDs.
        let ids = self.bpe.encode_ordinary(s);

        // If the string fits within the token limit, return it as-is.
        if ids.len() <= max_tokens {
            return (s.to_string(), ids.len());
        }

        // Otherwise, take only the allowed prefix of tokens.
        let prefix = &ids[..max_tokens];
        let text = self.bpe.decode(prefix.to_vec()).unwrap_or_else(|_| {
            // Log decoding failure and return partial content
            eprintln!("Warning: BPE decoding failed for token prefix, returning empty content");
            String::new()
        });
        let mut text = text.trim_end().to_string();

        // Ensure the trimmed text ends with a newline.
        if !text.ends_with('\n') {
            text.push('\n');
        }

        // Append ellipsis to indicate trimming.
        text.push_str("…\n");

        (text, max_tokens)
    }
}

/// Symbol relevance calculator for context assembly
#[derive(Debug, Clone)]
pub struct SymbolRanker {
    /// Anchor file for proximity calculations
    anchor_file: Option<std::path::PathBuf>,
    /// Anchor line for fine-grained proximity
    anchor_line: Option<usize>,
}

impl SymbolRanker {
    /// Create a new symbol ranker with optional anchor
    pub fn new(anchor_file: Option<&std::path::Path>, anchor_line: Option<usize>) -> Self {
        Self {
            anchor_file: anchor_file.map(|p| p.to_path_buf()),
            anchor_line,
        }
    }

    /// Calculate priority for a symbol based on multiple factors
    pub fn calculate_priority(
        &self,
        symbol: &crate::core::symbols::Symbol,
        query: &str,
        context_factors: &ContextFactors,
    ) -> Priority {
        let level = self.calculate_level(symbol, context_factors);
        let relevance = self.calculate_relevance(symbol, query);
        let proximity = self.calculate_proximity(symbol);

        Priority::custom(level, relevance, proximity)
    }

    /// Calculate base priority level (0-255)
    fn calculate_level(&self, symbol: &crate::core::symbols::Symbol, factors: &ContextFactors) -> u8 {
        let mut score = 100u8; // Start with medium baseline

        // Boost for public symbols (API surface)
        if matches!(symbol.visibility, Some(crate::core::symbols::Visibility::Public)) {
            score = score.saturating_add(30);
        }

        // Boost for important symbol kinds
        match symbol.kind {
            crate::core::symbols::SymbolKind::Function => score = score.saturating_add(20),
            crate::core::symbols::SymbolKind::Class | crate::core::symbols::SymbolKind::Struct => {
                score = score.saturating_add(25)
            }
            crate::core::symbols::SymbolKind::Module => score = score.saturating_add(15),
            _ => {}
        }

        // Boost for symbols in anchor file
        if let Some(ref anchor) = self.anchor_file && symbol.file.ends_with(anchor) {
            score = score.saturating_add(50);
        }

        // Boost for recently accessed symbols
        if factors.recently_accessed {
            score = score.saturating_add(20);
        }

        // Penalty for test files (usually less relevant for context)
        if symbol.file.to_string_lossy().contains("test") {
            score = score.saturating_sub(30);
        }

        score
    }

    /// Calculate relevance score based on query match quality (0.0-1.0)
    fn calculate_relevance(&self, symbol: &crate::core::symbols::Symbol, query: &str) -> f32 {
        let name = &symbol.name;
        let qualified_name = symbol.qualified_name.as_str();

        // Exact match is perfect
        if name == query || qualified_name == query {
            return 1.0;
        }

        // Prefix match is very good
        if name.starts_with(query) || qualified_name.starts_with(query) {
            return 0.9;
        }

        // Substring match is good
        if name.contains(query) || qualified_name.contains(query) {
            return 0.7;
        }

        // Case-insensitive fuzzy matching
        let query_lower = query.to_lowercase();
        let name_lower = name.to_lowercase();
        let qualified_lower = qualified_name.to_lowercase();

        if name_lower.contains(&query_lower) || qualified_lower.contains(&query_lower) {
            return 0.5;
        }

        // Fuzzy/partial matching (simple implementation)
        let score = fuzzy_match_score(&query_lower, &name_lower)
            .max(fuzzy_match_score(&query_lower, &qualified_lower));

        score * 0.4 // Scale down fuzzy matches
    }

    /// Calculate proximity score based on location relative to anchor (0.0-1.0)
    fn calculate_proximity(&self, symbol: &crate::core::symbols::Symbol) -> f32 {
        let Some(ref anchor_file) = self.anchor_file else {
            return 0.5; // No anchor = neutral proximity
        };

        // Same file = highest proximity
        if symbol.file.ends_with(anchor_file) {
            // If we have anchor line, consider line distance
            if let Some(anchor_line) = self.anchor_line {
                let line_distance = (symbol.start_line as i32 - anchor_line as i32).abs() as f32;
                // Closer lines get higher scores (allow deeper range as suggested)
                return (1.0_f32 - (line_distance / 1000.0).min(0.6)).max(0.4);
            }
            return 1.0;
        }

        // Same directory = good proximity
        if let (Some(symbol_parent), Some(anchor_parent)) =
            (symbol.file.parent(), anchor_file.parent())
        {
            if symbol_parent == anchor_parent {
                return 0.7;
            }

            // Calculate directory distance
            let symbol_components: Vec<_> = symbol_parent.components().collect();
            let anchor_components: Vec<_> = anchor_parent.components().collect();

            let common_prefix = symbol_components
                .iter()
                .zip(anchor_components.iter())
                .take_while(|(a, b)| a == b)
                .count();

            let total_depth = symbol_components.len().max(anchor_components.len());
            if total_depth > 0 {
                return (common_prefix as f32 / total_depth as f32) * 0.6;
            }
        }

        0.2 // Different directories = low proximity
    }
}

/// Additional context factors for ranking
#[derive(Debug, Clone, Default)]
pub struct ContextFactors {
    /// Whether this symbol was recently accessed
    pub recently_accessed: bool,
    /// Template context (refactor, bugfix, feature)
    pub template: Option<crate::cli::ContextTemplate>,
    /// Current phase of development
    pub development_phase: Option<DevelopmentPhase>,
}

/// Development phase affects symbol importance
#[derive(Debug, Clone, Copy)]
pub enum DevelopmentPhase {
    /// Early development - focus on core types and public APIs
    Early,
    /// Feature development - focus on implementation details
    Feature,
    /// Bug fixing - focus on error handling and edge cases
    Bugfix,
    /// Refactoring - focus on structure and organization
    Refactor,
}

/// Simple fuzzy matching score (0.0-1.0)
fn fuzzy_match_score(pattern: &str, text: &str) -> f32 {
    if pattern.is_empty() || text.is_empty() {
        return 0.0;
    }

    let mut pattern_chars = pattern.chars().peekable();
    let mut text_chars = text.chars();
    let mut matches = 0;
    let mut consecutive_matches = 0;
    let mut max_consecutive = 0;

    while let Some(pattern_char) = pattern_chars.peek() {
        if let Some(text_char) = text_chars.next() {
            if *pattern_char == text_char {
                pattern_chars.next();
                matches += 1;
                consecutive_matches += 1;
                max_consecutive = max_consecutive.max(consecutive_matches);
            } else {
                consecutive_matches = 0;
            }
        } else {
            break;
        }
    }

    if matches == 0 {
        return 0.0;
    }

    // Score based on match ratio with bonus for consecutive matches
    let match_ratio = matches as f32 / pattern.len() as f32;
    let consecutive_bonus = (max_consecutive as f32 / pattern.len() as f32) * 0.3;

    (match_ratio + consecutive_bonus).min(1.0)
}
