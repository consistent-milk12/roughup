//! Robust parsing for "<path>:<ranges>" with Windows support.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// Single extraction target: one file + merged line ranges.
#[derive(Debug, Clone)]
pub struct ExtractionTarget
{
    /// File path as provided (for display/ordering).
    pub file: PathBuf,
    /// Inclusive 1-based line ranges, merged and sorted.
    pub ranges: Vec<(usize, usize)>,
}

impl ExtractionTarget
{
    /// Parse a target string like
    /// "src/main.rs:1-5,10-15" or "C:\\src\\lib.rs:20-25".
    ///
    /// # Errors
    ///
    /// Returns an error if the input string is missing a file path or range spec,
    /// if any range is invalid, or if no valid ranges are found.
    pub fn parse(input: &str) -> Result<Self>
    {
        // Normalize and trim surrounding whitespace
        let s = input.trim();

        // Split from the right once to avoid breaking "C:\..."
        // This yields: [<ranges>, <path>] in reverse order.
        let mut it = s.rsplitn(2, ':');

        // Extract the tail part: ranges spec
        let ranges_str = it
            .next()
            .context("missing range spec after ':'")?
            .trim();

        // Extract the head part: path string
        let path_str = it
            .next()
            .context("missing file path before ':'")?
            .trim();

        // Build a PathBuf preserving the user's spelling
        let file = PathBuf::from(path_str);

        // Parse the comma-separated ranges
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for seg in ranges_str.split(',')
        {
            let seg = seg.trim();
            if seg.is_empty()
            {
                continue;
            }
            // Support "N" and "A-B" patterns
            if let Some((a, b)) = seg.split_once('-')
            {
                let a: usize = a
                    .trim()
                    .parse()
                    .with_context(|| format!("invalid start: {seg}"))?;
                let b: usize = b
                    .trim()
                    .parse()
                    .with_context(|| format!("invalid end: {seg}"))?;

                if a == 0 || b == 0
                {
                    bail!("line numbers must be >= 1: {seg}");
                }

                if a > b
                {
                    bail!("start > end in range: {seg}");
                }
                ranges.push((a, b));
            }
            else
            {
                let n: usize = seg
                    .parse()
                    .with_context(|| format!("invalid line: {seg}"))?;
                if n == 0
                {
                    bail!("line numbers must be >= 1: {seg}");
                }
                ranges.push((n, n));
            }
        }

        // Require at least one range
        if ranges.is_empty()
        {
            bail!("no valid ranges in: {input}");
        }

        // Merge and sort to avoid redundant work
        ranges.sort_unstable_by_key(|r| r.0);
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());

        for (s, e) in ranges
        {
            if let Some(last) = merged.last_mut()
                && s <= last.1 + 1
            {
                last.1 = last
                    .1
                    .max(e);
                continue;
            }

            merged.push((s, e));
        }

        // Emit normalized target
        Ok(Self { file, ranges: merged })
    }
}
