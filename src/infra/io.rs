use anyhow::{Context, Result};
use memmap2::Mmap;
use std::fs::File;
use std::path::Path;

const MMAP_THRESHOLD: u64 = 1024 * 1024; // 1 MiB

pub enum FileContent {
    Mapped(Mmap),
    Buffered(String),
}

impl AsRef<str> for FileContent {
    fn as_ref(&self) -> &str {
        match self {
            FileContent::Mapped(mmap) => {
                // Safety: We assume the file contains valid UTF-8
                // In production, we should handle invalid UTF-8 gracefully
                std::str::from_utf8(mmap).unwrap_or("")
            }
            FileContent::Buffered(s) => s.as_str(),
        }
    }
}

pub fn read_file_smart<P: AsRef<Path>>(path: P) -> Result<FileContent> {
    let path = path.as_ref();
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata for {}", path.display()))?;

    if metadata.len() > MMAP_THRESHOLD {
        // Use memory mapping for large files
        let file =
            File::open(path).with_context(|| format!("Failed to open file {}", path.display()))?;

        // Safety: We're only reading the file, not modifying it
        let mmap = unsafe { Mmap::map(&file) }
            .with_context(|| format!("Failed to memory-map {}", path.display()))?;

        Ok(FileContent::Mapped(mmap))
    } else {
        // Read small files into memory
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file {}", path.display()))?;

        Ok(FileContent::Buffered(content))
    }
}

/// Extract inclusive 1-based line ranges as a single String.
/// Ranges must be validated and merged by the caller.
pub fn extract_lines(content: &str, ranges: &[(usize, usize)]) -> Result<String> {
    // Work in bytes; validate once then slice cheaply
    let bytes = content.as_bytes();

    // Build index once per file
    let idx = crate::infra::line_index::NewlineIndex::build(bytes);

    // Short-circuit empty files
    if idx.line_count() == 0 {
        return Ok(String::new());
    }

    // Estimate capacity to reduce reallocations
    // (heuristic: ~60 bytes per line per range)
    let mut out = String::with_capacity(ranges.len() * 60);

    // Append each range in order, separating ranges with '\n'
    for (i, &(s, e)) in ranges.iter().enumerate() {
        // Validate line bounds
        if s == 0 || s > e || s > idx.line_count() {
            anyhow::bail!("invalid range: {s}-{e}");
        }

        // Clamp end to available lines
        let end = e.min(idx.line_count());

        // Map to byte span (exclusive end)
        let (lo, hi) = idx
            .byte_range_for_lines(s, end, bytes)
            .ok_or_else(|| anyhow::anyhow!("range out of bounds: {s}-{end}"))?;

        // Push the slice as UTF-8 (content is already valid)
        out.push_str(&content[lo..hi]);

        // Separate consecutive ranges with a single newline
        if i + 1 != ranges.len() {
            out.push('\n');
        }
    }

    // Return the composed buffer
    Ok(out)
}

pub fn merge_overlapping_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }

    // Sort by start position
    ranges.sort_by_key(|&(start, _)| start);

    let mut merged = vec![ranges[0]];

    for &(start, end) in &ranges[1..] {
        let last_idx = merged.len() - 1;
        let (last_start, last_end) = merged[last_idx];

        if start <= last_end + 1 {
            // Overlapping or adjacent ranges - merge them
            merged[last_idx] = (last_start, end.max(last_end));
        } else {
            // Non-overlapping range
            merged.push((start, end));
        }
    }

    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_overlapping_ranges() {
        assert_eq!(
            merge_overlapping_ranges(vec![(1, 3), (2, 5), (7, 9)]),
            vec![(1, 5), (7, 9)]
        );

        assert_eq!(
            merge_overlapping_ranges(vec![(1, 2), (3, 4)]),
            vec![(1, 4)] // Adjacent ranges should merge
        );

        assert_eq!(
            merge_overlapping_ranges(vec![(1, 1), (3, 3), (5, 5)]),
            vec![(1, 1), (3, 3), (5, 5)]
        );
    }

    #[test]
    fn test_extract_lines() {
        let content = "line1\nline2\nline3\nline4\nline5";

        let result = extract_lines(content, &[(2, 3)]).unwrap();
        assert_eq!(result, "line2\nline3");

        let result = extract_lines(content, &[(1, 2), (4, 5)]).unwrap();
        assert_eq!(result, "line1\nline2\nline4\nline5");
    }
}
