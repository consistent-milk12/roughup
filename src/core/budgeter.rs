use anyhow::{Context, Result, anyhow};
use moka::sync::Cache;
use std::collections::HashMap;
use tiktoken_rs::{CoreBPE, cl100k_base, get_bpe_from_model, o200k_base};
use xxhash_rust::xxh64::Xxh64;

/// Priority buckets for deterministic selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Highest priority bucket
    High = 2,

    /// Medium priority bucket  
    Medium = 1,

    /// Lowest priority bucket
    Low = 0,
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

        // Final tally
        let total_tokens: usize = out.iter().map(|x| x.tokens).sum();
        if total_tokens > budget_tokens {
            // As a last resort, shrink from lowest priority tail preserving determinism
            let mut i = out.len();
            let mut budget = budget_tokens;
            // Compute ids -> priorities map to shrink tail first on low-priority
            let mut pr_map: HashMap<String, Priority> = HashMap::new();
            for it in &hard_first {
                pr_map.insert(it.id.clone(), it.priority);
            }
            // Non-hard default to their original order priority; safer: treat missing as Low
            while i > 0 {
                i -= 1;
                let _p = pr_map.get(&out[i].id).copied().unwrap_or(Priority::Low);

                if out[i].tokens <= budget {
                    budget -= out[i].tokens;
                    continue;
                }

                // Need to trim this item to fit the remaining budget
                let keep = budget;
                let (s, t) = self.take_prefix(&out[i].content, keep);
                out[i].content = s;
                out[i].tokens = t;

                break;
            }
        }

        let total_tokens: usize = out.iter().map(|x| x.tokens).sum();

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
        let text = self.bpe.decode(prefix.to_vec()).unwrap_or_default();
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
