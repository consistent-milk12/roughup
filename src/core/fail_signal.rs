//! Fail-Signal Seeding (Phase 3.5 - Week 3)
//!
//! Parse compiler/test logs to extract failing files, lines, symbols, and messages.
//! Feed these into context assembly to boost nearby spans and pull in callsites.

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Severity level of a fail signal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity
{
    Info,
    Warn,
    Error,
}

/// A parsed failure signal from compiler/test logs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailSignal
{
    /// File path where the failure occurred
    pub file: PathBuf,
    /// 1-based line numbers where failures occurred
    pub line_hits: Vec<usize>,
    /// Parsed identifiers/symbols from the failure
    pub symbols: Vec<String>,
    /// Short failure reason/message
    pub message: String,
    /// Severity level
    pub severity: Severity,
}

/// Trait for parsing different log formats
pub trait FailSignalParser
{
    /// Parse log text into fail signals
    fn parse(
        &self,
        text: &str,
    ) -> Vec<FailSignal>;
    /// Format identifier for this parser
    fn format(&self) -> &'static str;
}

/// Rustc/Cargo error parser with stateful severity/message attribution
pub struct RustcParser;

impl FailSignalParser for RustcParser
{
    fn parse(
        &self,
        text: &str,
    ) -> Vec<FailSignal>
    {
        let mut out = Vec::new();
        let mut cur_sev = Severity::Info;
        let mut cur_msg = String::new();

        for line in text.lines()
        {
            let l = line.trim_start();

            // 1) Header lines - detect severity and message
            if l.starts_with("error[") || l.starts_with("error:")
            {
                cur_sev = Severity::Error;
                cur_msg = l.to_string();
                continue;
            }
            if l.starts_with("warning[") || l.starts_with("warning:")
            {
                cur_sev = Severity::Warn;
                cur_msg = l.to_string();
                continue;
            }

            // 2) Arrow site - emit signal using stored severity/message
            if let Some((file, line_no, _col)) = parse_rustc_arrow(l)
            {
                out.push(FailSignal {
                    file: PathBuf::from(file),
                    line_hits: vec![line_no],
                    symbols: extract_rust_symbols(&cur_msg),
                    message: truncate_msg(&cur_msg),
                    severity: cur_sev,
                });
                continue;
            }

            // 3) Reset on fence or empty line
            if l == "|" || l.is_empty()
            {
                cur_sev = Severity::Info;
                cur_msg.clear();
            }
        }

        merge_and_sort_signals(out)
    }

    fn format(&self) -> &'static str
    {
        "rustc"
    }
}

/// Pytest error parser with improved message extraction
pub struct PytestParser;

impl FailSignalParser for PytestParser
{
    fn parse(
        &self,
        text: &str,
    ) -> Vec<FailSignal>
    {
        let mut out = Vec::new();
        let lines: Vec<_> = text
            .lines()
            .collect();

        for i in 0..lines.len()
        {
            let l = lines[i].trim_start();
            if !l.starts_with("File \"")
            {
                continue;
            }

            if let Some((file, line_no)) = parse_py_file_line(l)
            {
                // Look ahead for better message context
                let msg = lines
                    .get(i + 1)
                    .map(|s| s.trim())
                    .unwrap_or_default();
                let tail = lines
                    .get(i + 2)
                    .map(|s| s.trim())
                    .unwrap_or_default();
                let message = if tail.eq_ignore_ascii_case("assertionerror")
                {
                    "AssertionError".to_string()
                }
                else if !msg.is_empty()
                {
                    msg.to_string()
                }
                else
                {
                    "Pytest failure".to_string()
                };

                out.push(FailSignal {
                    file: PathBuf::from(file),
                    line_hits: vec![line_no],
                    symbols: extract_python_symbols(l),
                    message: truncate_msg(&message),
                    severity: Severity::Error,
                });
            }
        }

        merge_and_sort_signals(out)
    }

    fn format(&self) -> &'static str
    {
        "pytest"
    }
}

/// Jest error parser with parenthesized location support
pub struct JestParser;

impl FailSignalParser for JestParser
{
    fn parse(
        &self,
        text: &str,
    ) -> Vec<FailSignal>
    {
        let mut out = Vec::new();

        for raw in text.lines()
        {
            let l = raw.trim_start();
            if !l.starts_with("at ")
            {
                continue;
            }

            // Handle both "at func (path:line:col)" and "at path:line:col"
            let loc = if let (Some(lp), Some(rp)) = (l.rfind('('), l.rfind(')'))
            {
                // Extract from parentheses: "at func (path:line:col)"
                if rp > lp { &l[lp + 1..rp] } else { &l[3..] }
            }
            else
            {
                // Bare format: "at path:line:col"
                &l[3..]
            }
            .trim();

            if let Some((file, line_no, _col)) = split_file_line_col(loc)
            {
                out.push(FailSignal {
                    file: PathBuf::from(file),
                    line_hits: vec![line_no],
                    symbols: extract_js_symbols(raw),
                    message: "Jest test failure".to_string(),
                    severity: Severity::Error,
                });
            }
        }

        merge_and_sort_signals(out)
    }

    fn format(&self) -> &'static str
    {
        "jest"
    }
}

/// Auto-detect format and parse
pub fn parse_fail_signals(
    text: &str,
    format: Option<&str>,
) -> Result<Vec<FailSignal>>
{
    let parsers: Vec<Box<dyn FailSignalParser>> =
        vec![Box::new(RustcParser), Box::new(PytestParser), Box::new(JestParser)];

    if let Some(format_name) = format
    {
        // Use specific parser
        for parser in &parsers
        {
            if parser.format() == format_name
            {
                return Ok(parser.parse(text));
            }
        }
        bail!("Unknown fail-signal format: {}", format_name);
    }

    // Auto-detect: try parsers in order, return first with results
    for parser in &parsers
    {
        let signals = parser.parse(text);
        if !signals.is_empty()
        {
            return Ok(signals);
        }
    }

    Ok(Vec::new())
}

// Helper functions for robust parsing

/// Parse rustc arrow line: "--> path:line:col" or "--> path:line"
/// Handles Windows paths by parsing from the end
fn parse_rustc_arrow(line: &str) -> Option<(&str, usize, Option<usize>)>
{
    let arrow = line
        .strip_prefix("-->")?
        .trim();

    // Find the last colon (could be line or column)
    let last_colon = arrow.rfind(':')?;
    let (before_last, after_last) = arrow.split_at(last_colon);
    let after_last = &after_last[1..]; // skip ':'

    // Check if there's a second colon (indicating path:line:col format)
    if let Some(second_colon) = before_last.rfind(':')
    {
        let (path_part, line_part) = before_last.split_at(second_colon);
        let line_part = &line_part[1..]; // skip ':'

        if let (Ok(line_no), Ok(col)) = (line_part.parse::<usize>(), after_last.parse::<usize>())
        {
            // path:line:col format
            Some((path_part, line_no, Some(col)))
        }
        else
        {
            None
        }
    }
    else
    {
        // Only one colon, so it's path:line format
        if let Ok(line_no) = after_last.parse::<usize>()
        {
            Some((before_last, line_no, None))
        }
        else
        {
            None
        }
    }
}

/// Parse pytest file line: 'File "/path", line N, in func'
fn parse_py_file_line(l: &str) -> Option<(&str, usize)>
{
    let start = l.find("File \"")? + 6;
    let rest = &l[start..];
    let end = rest.find('"')?;
    let file = &rest[..end];
    let after = &rest[end + 1..];
    let line_pos = after.find("line ")? + 5;
    let after_line = &after[line_pos..];
    let num_end = after_line
        .find(',')
        .unwrap_or(after_line.len());
    let line_no = after_line[..num_end]
        .trim()
        .parse::<usize>()
        .ok()?;
    Some((file, line_no))
}

/// Robust split from the end to handle Windows paths: "C:\path\file.js:10:5"
fn split_file_line_col(s: &str) -> Option<(&str, usize, Option<usize>)>
{
    let last = s.rfind(':')?;
    let (pre, right) = s.split_at(last);
    let right = &right[1..];
    if let Ok(col) = right.parse::<usize>()
    {
        let mid = pre.rfind(':')?;
        let (file, line_s) = pre.split_at(mid);
        let line_no = line_s[1..]
            .parse::<usize>()
            .ok()?;
        Some((file, line_no, Some(col)))
    }
    else if let Ok(line_no) = right.parse::<usize>()
    {
        Some((pre, line_no, None))
    }
    else
    {
        None
    }
}

/// Merge duplicate signals by (file,line) and sort deterministically
fn merge_and_sort_signals(mut v: Vec<FailSignal>) -> Vec<FailSignal>
{
    use std::collections::BTreeMap;
    let mut map: BTreeMap<(String, usize), FailSignal> = BTreeMap::new();

    for mut s in v.drain(..)
    {
        let key = (
            s.file
                .to_string_lossy()
                .to_string(),
            *s.line_hits
                .first()
                .unwrap_or(&0),
        );
        map.entry(key)
            .and_modify(|e| {
                // Merge symbols (dedup), keep highest severity, concatenate message once
                let mut set: std::collections::BTreeSet<_> = e
                    .symbols
                    .iter()
                    .cloned()
                    .collect();
                set.extend(
                    s.symbols
                        .drain(..),
                );
                e.symbols = set
                    .into_iter()
                    .collect();
                if s.severity as u8 > e.severity as u8
                {
                    e.severity = s.severity;
                }
                if e.message
                    .is_empty()
                    && !s
                        .message
                        .is_empty()
                {
                    e.message = s
                        .message
                        .clone();
                }
            })
            .or_insert(s);
    }

    let mut out: Vec<_> = map
        .into_values()
        .collect();
    out.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line_hits[0].cmp(&b.line_hits[0]))
    });
    out
}

/// Truncate message to reasonable length
fn truncate_msg(m: &str) -> String
{
    if m.len() > 160
    {
        format!("{}…", &m[..157])
    }
    else
    {
        m.to_string()
    }
}

// Helper functions for symbol extraction

fn extract_rust_symbols(line: &str) -> Vec<String>
{
    let mut symbols = Vec::new();

    // Simple identifier extraction - look for Rust patterns
    for word in line.split_whitespace()
    {
        if word
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == ':')
            && word
                .chars()
                .any(|c| c.is_alphabetic())
        {
            symbols.push(word.to_string());
        }
    }

    symbols
}

fn extract_python_symbols(line: &str) -> Vec<String>
{
    let mut symbols = Vec::new();

    // Look for "in function_name" pattern
    if let Some(in_pos) = line.find(" in ")
    {
        let after_in = &line[in_pos + 4..];
        if let Some(func_end) = after_in.find(' ')
        {
            let func_name = &after_in[..func_end];
            symbols.push(func_name.to_string());
        }
    }

    symbols
}

fn extract_js_symbols(line: &str) -> Vec<String>
{
    let mut symbols = Vec::new();

    // Extract function names from stack traces
    if let Some(at_pos) = line.find(" at ")
    {
        let after_at = &line[at_pos + 4..];
        if let Some(space_pos) = after_at.find(' ')
        {
            let func_part = &after_at[..space_pos];
            symbols.push(func_part.to_string());
        }
    }

    symbols
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn test_rustc_severity_and_message()
    {
        let log = r#"
error[E0308]: mismatched types
  --> src/main.rs:10:5
   |
10 |     "hello"
   |     ^^^^^^^ expected `i32`, found `&str`
"#;

        let parser = RustcParser;
        let signals = parser.parse(log);

        assert_eq!(signals.len(), 1);
        let signal = &signals[0];
        assert_eq!(signal.file, PathBuf::from("src/main.rs"));
        assert_eq!(signal.line_hits, vec![10]);
        assert_eq!(signal.severity, Severity::Error);
        assert!(
            signal
                .message
                .starts_with("error[E0308]: mismatched types")
        );
    }

    #[test]
    fn test_rustc_without_column()
    {
        let log = r#"
warning: unused variable
  --> src/lib.rs:123
"#;

        let parser = RustcParser;
        let signals = parser.parse(log);

        assert_eq!(signals.len(), 1);
        let signal = &signals[0];
        assert_eq!(signal.file, PathBuf::from("src/lib.rs"));
        assert_eq!(signal.line_hits, vec![123]);
        assert_eq!(signal.severity, Severity::Warn);
    }

    #[test]
    fn test_rustc_windows_path()
    {
        let log = r#"
error: cannot find macro
  --> C:\proj\file.rs:10:5
"#;

        let parser = RustcParser;
        let signals = parser.parse(log);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].file, PathBuf::from("C:\\proj\\file.rs"));
        assert_eq!(signals[0].line_hits, vec![10]);
    }

    #[test]
    fn test_jest_with_parentheses()
    {
        let log = "    at it (/repo/app.test.js:42:7)";

        let parser = JestParser;
        let signals = parser.parse(log);

        assert_eq!(signals.len(), 1);
        let signal = &signals[0];
        assert_eq!(signal.file, PathBuf::from("/repo/app.test.js"));
        assert_eq!(signal.line_hits, vec![42]);
    }

    #[test]
    fn test_jest_bare_format()
    {
        let log = "    at /repo/app.test.js:42:7";

        let parser = JestParser;
        let signals = parser.parse(log);

        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].file, PathBuf::from("/repo/app.test.js"));
        assert_eq!(signals[0].line_hits, vec![42]);
    }

    #[test]
    fn test_pytest_assertion_message()
    {
        let log = r#"
    File "/home/user/test.py", line 42, in test_function
        assert x == 5
AssertionError
"#;

        let parser = PytestParser;
        let signals = parser.parse(log);

        assert_eq!(signals.len(), 1);
        let signal = &signals[0];
        assert_eq!(signal.file, PathBuf::from("/home/user/test.py"));
        assert_eq!(signal.line_hits, vec![42]);
        assert_eq!(signal.message, "AssertionError");
        assert_eq!(signal.severity, Severity::Error);
    }

    #[test]
    fn test_deduping_and_sort_stability()
    {
        let log = r#"
error[E0308]: first error
  --> src/main.rs:10:5
error[E0277]: second error  
  --> src/main.rs:10:8
"#;

        let parser = RustcParser;
        let signals = parser.parse(log);

        // Should merge into single signal with highest severity
        assert_eq!(signals.len(), 1);
        let signal = &signals[0];
        assert_eq!(signal.file, PathBuf::from("src/main.rs"));
        assert_eq!(signal.line_hits, vec![10]);
        assert_eq!(signal.severity, Severity::Error);
    }

    #[test]
    fn test_auto_detection()
    {
        let rustc_log = "error[E0308]: mismatched types\n  --> src/main.rs:10:5";
        let signals = parse_fail_signals(rustc_log, None).unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].file, PathBuf::from("src/main.rs"));
    }

    #[test]
    fn test_message_truncation()
    {
        let very_long_msg = "a".repeat(200);
        let truncated = truncate_msg(&very_long_msg);
        assert!(truncated.len() <= 160);
        assert!(truncated.ends_with('…'));
    }
}
