use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;

/// Token-aware chunking backend using HF tokenizers (default) or tiktoken (feature-gated)
pub struct ChunkingBackend {
    tokenizer: Tokenizer,
    model_name: String,
}

impl ChunkingBackend {
    /// Create backend with HF tokenizer for specified model
    pub fn new_hf(model_name: &str) -> Result<Self> {
        // Common HF model mappings
        let tokenizer_name = match model_name {
            "gpt2" | "code-davinci" => "gpt2",
            "claude" | "claude-3" => "claude", // Anthropic tokenizer
            "llama2" | "llama" => "meta-llama/Llama-2-7b-hf",
            "codellama" => "codellama/CodeLlama-7b-Python-hf", 
            _ => model_name, // Pass through custom model names
        };

        let tokenizer = Tokenizer::from_pretrained(tokenizer_name, None)
            .context("Failed to load HF tokenizer")?;

        Ok(Self { 
            tokenizer, 
            model_name: model_name.to_string(),
        })
    }

    #[cfg(feature = "tiktoken")]
    /// Create backend with tiktoken for OpenAI compatibility
    pub fn new_tiktoken(model_name: &str) -> Result<Self> {
        // tiktoken backend implementation when feature enabled
        let encoding = tiktoken_rs::get_bpe_from_model(model_name)
            .context("Failed to get tiktoken encoding")?;
        
        // Wrap tiktoken in HF tokenizer interface
        unimplemented!("tiktoken backend - use HF tokenizers as default")
    }

    /// Count tokens in text efficiently 
    pub fn count_tokens(&self, text: &str) -> Result<usize> {
        let encoding = self.tokenizer.encode(text, false)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;
        Ok(encoding.len())
    }

    /// Split text into token-aware chunks with overlap
    pub fn chunk_with_overlap(
        &self, 
        text: &str, 
        max_tokens: usize, 
        overlap_tokens: usize
    ) -> Result<Vec<String>> {
        let encoding = self.tokenizer.encode(text, false)
            .map_err(|e| anyhow::anyhow!("Encoding failed: {}", e))?;
        
        let token_ids = encoding.get_ids();
        
        if token_ids.len() <= max_tokens {
            return Ok(vec![text.to_string()]);
        }

        let mut chunks = Vec::new();
        let mut start = 0;

        while start < token_ids.len() {
            let end = (start + max_tokens).min(token_ids.len());
            
            // Decode token range back to text
            let chunk_tokens = &token_ids[start..end];
            let chunk_text = self.tokenizer.decode(chunk_tokens, false)
                .map_err(|e| anyhow::anyhow!("Decode failed: {}", e))?;
                
            chunks.push(chunk_text);
            
            if end >= token_ids.len() {
                break;
            }
            
            // Move start forward with overlap
            start = end.saturating_sub(overlap_tokens);
        }

        Ok(chunks)
    }
}

/// Symbol-aware chunking strategy - prefer function/class boundaries
pub fn chunk_by_symbols(
    content: &str,
    symbols: &[crate::core::symbols::Symbol],
    max_tokens: usize,
    backend: &ChunkingBackend,
) -> Result<Vec<ChunkInfo>> {
    let mut chunks = Vec::new();
    let mut current_tokens = 0;
    let mut current_symbols = Vec::new();
    
    for symbol in symbols {
        let symbol_text = extract_symbol_text(content, symbol)?;
        let token_count = backend.count_tokens(&symbol_text)?;
        
        // If adding this symbol would exceed limit, finalize current chunk
        if current_tokens + token_count > max_tokens && !current_symbols.is_empty() {
            chunks.push(ChunkInfo::from_symbols(content, &current_symbols, backend)?);
            current_symbols.clear();
            current_tokens = 0;
        }
        
        // If single symbol exceeds limit, split it with token-based chunking
        if token_count > max_tokens {
            let sub_chunks = backend.chunk_with_overlap(&symbol_text, max_tokens, 128)?;
            for (i, sub_chunk) in sub_chunks.into_iter().enumerate() {
                chunks.push(ChunkInfo::new(
                    sub_chunk,
                    format!("{}[part_{}]", symbol.qualified_name, i + 1),
                    symbol.file.clone(),
                    symbol.start_line,
                    symbol.end_line,
                ));
            }
        } else {
            current_symbols.push(symbol.clone());
            current_tokens += token_count;
        }
    }
    
    // Add remaining symbols as final chunk
    if !current_symbols.is_empty() {
        chunks.push(ChunkInfo::from_symbols(content, &current_symbols, backend)?);
    }
    
    Ok(chunks)
}

/// Chunk metadata for LLM consumption
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    pub content: String,
    pub symbol_path: String,
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub token_count: usize,
}

impl ChunkInfo {
    pub fn new(
        content: String, 
        symbol_path: String, 
        file: PathBuf, 
        start_line: usize, 
        end_line: usize
    ) -> Self {
        Self {
            token_count: 0, // Will be computed later
            content,
            symbol_path,
            file,
            start_line,
            end_line,
        }
    }
    
    pub fn from_symbols(
        content: &str,
        symbols: &[crate::core::symbols::Symbol],
        backend: &ChunkingBackend,
    ) -> Result<Self> {
        let mut chunk_content = String::new();
        let mut symbol_names = Vec::new();
        let mut min_line = usize::MAX;
        let mut max_line = 0;
        let file = symbols[0].file.clone();
        
        for symbol in symbols {
            let symbol_text = extract_symbol_text(content, symbol)?;
            chunk_content.push_str(&symbol_text);
            chunk_content.push('\n');
            symbol_names.push(symbol.qualified_name.clone());
            min_line = min_line.min(symbol.start_line);
            max_line = max_line.max(symbol.end_line);
        }
        
        let token_count = backend.count_tokens(&chunk_content)?;
        let symbol_path = symbol_names.join(", ");
        
        Ok(Self {
            content: chunk_content,
            symbol_path,
            file,
            start_line: min_line,
            end_line: max_line,
            token_count,
        })
    }
}

/// Extract text for a specific symbol from file content
fn extract_symbol_text(content: &str, symbol: &crate::core::symbols::Symbol) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    
    if symbol.start_line == 0 || symbol.start_line > lines.len() {
        return Ok(String::new());
    }
    
    let start_idx = symbol.start_line.saturating_sub(1);
    let end_idx = symbol.end_line.min(lines.len());
    
    Ok(lines[start_idx..end_idx].join("\n"))
}