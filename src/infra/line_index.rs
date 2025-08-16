//! Filepath: src/infra/line_index.rs
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

use memchr::memchr_iter;

#[derive(Debug, Clone)]
pub struct NewlineIndex
{
    /// Total length of the buffer in bytes.
    len: usize,

    /// Positions of '\n' characters in the buffer.
    nl_positions: Vec<usize>,
}

impl NewlineIndex
{
    /// Build an index recording positions of '\n'.
    #[must_use]
    pub fn build(bytes: &[u8]) -> Self
    {
        // Pre-allocate space for newline positions (heuristic: 1 NL per 48 bytes)
        let mut nl_positions = Vec::with_capacity(bytes.len() / 48);

        // Find all '\n' positions using memchr for efficiency
        nl_positions.extend(memchr_iter(b'\n', bytes));

        // Construct the NewlineIndex with collected positions and buffer length
        Self { nl_positions, len: bytes.len() }
    }

    /// Total number of logical lines.
    /// Empty => 0. Non-empty => (# of '\n') + 1.
    /// Note: A trailing '\n' yields an additional empty last line.
    #[must_use] 
    pub fn line_count(&self) -> usize
    {
        if self.len == 0
        {
            0
        }
        else
        {
            self.nl_positions
                .len()
                + 1
        }
    }

    #[must_use] 
    pub fn start_byte_of_line(
        &self,
        line1: usize,
    ) -> Option<usize>
    {
        // Get the total number of lines in the buffer
        let total = self.line_count();

        // Return None if the requested line is out of bounds (0 or > total)
        if line1 == 0 || line1 > total
        {
            return None;
        }

        // The first line always starts at byte 0
        if line1 == 1
        {
            return Some(0);
        }

        // For other lines, start is just after the previous '\n'
        self.nl_positions
            .get(line1 - 2)
            .map(|&prev_nl| prev_nl + 1)
    }

    /// End byte (exclusive) of a 1-based line.
    /// For CRLF, excludes trailing '\r' before '\n'.
    #[must_use] 
    pub fn end_byte_of_line(
        &self,
        line1: usize,
        bytes: &[u8],
    ) -> Option<usize>
    {
        // Ensure the bytes slice matches the indexed buffer length
        debug_assert_eq!(
            bytes.len(),
            self.len,
            "bytes length must match indexed buffer length"
        );

        // Get the total number of lines in the buffer
        let total = self.line_count();

        // Return None if the requested line is out of bounds (0 or > total)
        if line1 == 0 || line1 > total
        {
            return None;
        }

        // If the line is not the last line, find the position of the corresponding '\n'
        if line1
            <= self
                .nl_positions
                .len()
        {
            let nl = self.nl_positions[line1 - 1];

            // For CRLF, exclude trailing '\r' before '\n'
            if nl > 0 && bytes.get(nl - 1) == Some(&b'\r')
            {
                return Some(nl - 1);
            }

            return Some(nl);
        }

        // Last line without trailing '\n' ends at EOF.
        Some(self.len)
    }

    /// Byte range (start..end) for a 1-based inclusive line span.
    #[must_use] 
    pub fn byte_range_for_lines(
        &self,
        start_line1: usize,
        end_line1: usize,
        bytes: &[u8],
    ) -> Option<(usize, usize)>
    {
        // Ensure the bytes slice matches the indexed buffer length
        debug_assert_eq!(
            bytes.len(),
            self.len,
            "bytes length must match indexed buffer length"
        );

        // Return None if either line is zero or start is after end
        if start_line1 == 0 || end_line1 == 0 || start_line1 > end_line1
        {
            return None;
        }

        // Get the total number of lines in the buffer
        let total = self.line_count();

        // Return None if buffer is empty
        if total == 0
        {
            return None;
        }

        // Get the start byte of the starting line
        let s = self.start_byte_of_line(start_line1)?;

        // Get the end byte of the ending line (clamped to total lines)
        let e = self.end_byte_of_line(cmp::min(end_line1, total), bytes)?;

        // Ensure the range is valid and within buffer bounds
        if s <= e && e <= self.len
        {
            Some((s, e))
        }
        else
        {
            None
        }
    }

    /// 1-based line number covering the given byte offset.
    /// Offsets at '\n' belong to the next line. Clamps byte > len to len.
    /// Returns 0 for empty buffers.
    #[must_use] 
    pub fn line_of_byte(
        &self,
        mut byte: usize,
    ) -> usize
    {
        // Return 0 for empty buffers
        if self.len == 0
        {
            return 0;
        }

        // Clamp byte offset to buffer length
        if byte > self.len
        {
            byte = self.len;
        }

        // Binary search for the line containing the byte offset
        match self
            .nl_positions
            .binary_search(&byte)
        {
            // If at '\n', belongs to next line
            Ok(pos) => pos + 2,

            // Otherwise, line is pos + 1
            Err(pos) => pos + 1,
        }
    }
}
