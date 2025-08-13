//! Newline index with LF/CRLF-robust line/byte mapping.
//!
//! Goals
//! - Single pass over bytes to record '\n' positions.
//! - 1-based external line numbers (friendly for UX).
//! - O(1) line→byte start/end via the index.
//! - End byte excludes trailing '\r' for CRLF lines.
//! - Binary search for byte→line mapping.
//!
//! Notes
//! - An empty buffer has 0 lines.
//! - A non-empty buffer without '\n' has 1 line.
//! - For ranges, end is exclusive (Rust slicing convention).

use std::cmp;

#[derive(Debug, Clone)]
pub struct NewlineIndex {
    /// Byte positions of every '\n' in the buffer.
    nl_positions: Vec<usize>,
    /// Total byte length of the buffer.
    len: usize,
}

impl NewlineIndex {
    /// Build an index recording positions of '\n'.
    pub fn build(bytes: &[u8]) -> Self {
        let mut nl_positions = Vec::with_capacity(bytes.len() / 48);
        let mut i = 0usize;

        // Single pass; record every '\n' offset.
        while let Some(pos) = memchr::memchr(b'\n', &bytes[i..]) {
            let abs = i + pos;
            nl_positions.push(abs);
            i = abs + 1;
        }

        Self {
            nl_positions,
            len: bytes.len(),
        }
    }

    /// Total number of logical lines.
    /// Empty buffer => 0 lines; else (#'\n' + 1).
    pub fn line_count(&self) -> usize {
        if self.len == 0 {
            0
        } else {
            self.nl_positions.len() + 1
        }
    }

    /// Start byte (inclusive) of a 1-based line.
    /// Returns None if line is out of range.
    pub fn start_byte_of_line(&self, line1: usize) -> Option<usize> {
        let total = self.line_count();
        if line1 == 0 || line1 > total {
            return None;
        }
        if line1 == 1 {
            return Some(0);
        }
        // For line L>1, start is one past the previous '\n'.
        self.nl_positions
            .get(line1 - 2)
            .map(|&prev_nl| prev_nl + 1)
    }

    /// End byte (exclusive) of a 1-based line.
    /// Returns None if line is out of range.
    /// For CRLF, excludes trailing '\r' before '\n'.
    pub fn end_byte_of_line(&self, line1: usize, bytes: &[u8]) -> Option<usize> {
        let total = self.line_count();
        if line1 == 0 || line1 > total {
            return None;
        }

        // Lines that end with '\n' (not the last line without NL)
        if line1 <= self.nl_positions.len() {
            let nl = self.nl_positions[line1 - 1];
            // If preceding byte is '\r', exclude it.
            if nl > 0 && bytes.get(nl.wrapping_sub(1)) == Some(&b'\r') {
                return Some(nl - 1);
            }
            return Some(nl);
        }

        // Last line without trailing '\n' ends at EOF.
        Some(self.len)
    }

    /// Byte range (start..end) for an inclusive 1-based line span.
    /// Returns None if the span is invalid or out of range.
    pub fn byte_range_for_lines(
        &self,
        start_line1: usize,
        end_line1: usize,
        bytes: &[u8],
    ) -> Option<(usize, usize)> {
        if start_line1 == 0 || end_line1 == 0 || start_line1 > end_line1 {
            return None;
        }
        let total = self.line_count();
        if total == 0 {
            return None;
        }

        let s = self.start_byte_of_line(start_line1)?;
        let e = self.end_byte_of_line(
            cmp::min(end_line1, total),
            bytes,
        )?;

        if s <= e && e <= self.len {
            Some((s, e))
        } else {
            None
        }
    }

    /// 1-based line number covering the given byte offset.
    /// Offsets at '\n' belong to the *next* line.
    /// Returns 0 for empty buffers.
    pub fn line_of_byte(&self, byte: usize) -> usize {
        if self.len == 0 {
            return 0;
        }
        // Count how many '\n' are strictly before `byte`.
        // upper_bound on nl_positions for `byte - 1`.
        let idx = match self.nl_positions.binary_search(&byte) {
            Ok(pos) => pos + 1, // at NL → next line
            Err(pos) => pos,    // number of NLs before `byte`
        };
        idx + 1
    }
}