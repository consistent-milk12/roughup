//! Line-range extraction with gitignore awareness and memory mapping.

pub mod target;

pub use target::ExtractionTarget;

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use rayon::prelude::*;
use std::path::Path;

use crate::cli::{AppContext, ExtractArgs};
use crate::infra::io::{extract_lines, read_file_smart};

pub fn run(args: ExtractArgs, ctx: &AppContext) -> Result<()> {
    // Parse extraction targets using robust Windows-aware parser
    let targets: Result<Vec<_>> = args
        .targets
        .iter()
        .map(|t| ExtractionTarget::parse(t))
        .collect();
    let targets = targets?;

    if targets.is_empty() {
        anyhow::bail!("No extraction targets specified");
    }

    // Set up progress bar (unless quiet mode)
    let progress = if ctx.quiet {
        ProgressBar::hidden()
    } else {
        let pb = ProgressBar::new(targets.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
                )
                .unwrap()
                .progress_chars("#>-"),
        );
        pb
    };

    // Process in parallel with order preserved in collect
    let pieces: Vec<Result<String>> = targets
        .par_iter()
        .map(|target| {
            // Read file (mmap > 1MB, else read_to_string)
            let content = read_file_smart(&target.file)
                .with_context(|| format!("Failed to read file: {}", target.file.display()))?;

            // Extract merged ranges with LF/CRLF-safe slicing
            let body = extract_lines(content.as_ref(), &target.ranges).with_context(|| {
                format!("Failed to extract lines from {}", target.file.display())
            })?;

            // Optional formatting (annotate/fence/…)
            let formatted = format_extraction(
                target.file.as_path(),
                &target.ranges,
                &body,
                args.annotate,
                args.fence,
            );

            // Update progress (best-effort, thread-safe)
            progress.inc(1);
            progress.set_message(format!("Processed {}", target.file.display()));

            Ok(formatted)
        })
        .collect();

    progress.finish_with_message("Extraction complete");

    // Combine results in original CLI order
    let mut final_content = String::new();
    for piece in pieces {
        let s = piece?;
        // Separate files with a single newline
        if !final_content.is_empty() {
            final_content.push('\n');
        }
        final_content.push_str(&s);
    }

    // Write output
    let dry_run = ctx.dry_run;
    if !dry_run {
        std::fs::write(&args.output, &final_content)
            .with_context(|| format!("Failed to write to {}", args.output.display()))?;
    }

    if args.clipboard && !dry_run {
        copy_to_clipboard(&final_content)?;
    }

    if dry_run {
        if !ctx.quiet {
            println!("{}", "DRY RUN: Would extract:".yellow());
            for target in &targets {
                println!("  {} (lines {:?})", target.file.display(), target.ranges);
            }
            println!(
                "{}",
                format!(
                    "Would write {} bytes to {}",
                    final_content.len(),
                    args.output.display()
                )
                .yellow()
            );
        }
    } else if !ctx.quiet {
        println!(
            "{} Extracted {} bytes to {}",
            "✓".green(),
            final_content.len(),
            args.output.display()
        );
    }

    Ok(())
}

fn format_extraction(
    file: &Path,
    ranges: &[(usize, usize)],
    content: &str,
    annotate: bool,
    fence: bool,
) -> String {
    let mut result = String::new();

    if annotate {
        let ranges_str = ranges
            .iter()
            .map(|(start, end)| {
                if start == end {
                    start.to_string()
                } else {
                    format!("{}-{}", start, end)
                }
            })
            .collect::<Vec<_>>()
            .join(",");

        result.push_str(&format!(
            "// File: {} (lines {})\n",
            file.display(),
            ranges_str
        ));
    }

    if fence {
        // Try to detect language from file extension
        let lang = file
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| match ext {
                "rs" => "rust",
                "py" => "python",
                "js" | "jsx" => "javascript",
                "ts" | "tsx" => "typescript",
                "go" => "go",
                "c" | "h" => "c",
                "cpp" | "cxx" | "cc" | "hpp" => "cpp",
                _ => ext,
            })
            .unwrap_or("");

        result.push_str(&format!("```{}\n", lang));
        result.push_str(content);
        if !content.ends_with('\n') {
            result.push('\n');
        }
        result.push_str("```\n");
    } else {
        result.push_str(content);
    }

    result
}

fn copy_to_clipboard(content: &str) -> Result<()> {
    use arboard::Clipboard;

    let mut clipboard = Clipboard::new().context("Failed to access clipboard")?;

    clipboard
        .set_text(content)
        .context("Failed to copy to clipboard")?;

    println!("{} Copied to clipboard", "✓".green());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extraction_target() {
        let target = ExtractionTarget::parse("src/main.rs:10-20,25,30-35").unwrap();
        assert_eq!(target.file, std::path::PathBuf::from("src/main.rs"));
        assert_eq!(target.ranges, vec![(10, 20), (25, 25), (30, 35)]);
    }

    #[test]
    fn test_parse_single_line() {
        let target = ExtractionTarget::parse("test.rs:42").unwrap();
        assert_eq!(target.ranges, vec![(42, 42)]);
    }

    #[test]
    fn test_parse_windows_path() {
        let target = ExtractionTarget::parse(r"C:\work\repo\src\lib.rs:3-4,10").unwrap();
        assert_eq!(
            target.file,
            std::path::PathBuf::from(r"C:\work\repo\src\lib.rs")
        );
        assert_eq!(target.ranges, vec![(3, 4), (10, 10)]);
    }

    #[test]
    fn test_parse_invalid_format() {
        assert!(ExtractionTarget::parse("invalid").is_err());
        assert!(ExtractionTarget::parse("file:invalid-range").is_err());
    }

    #[test]
    fn test_adjacent_range_merge() {
        let target = ExtractionTarget::parse("src/x.rs:1-5,6-10").unwrap();
        assert_eq!(target.ranges, vec![(1, 10)]); // Merged adjacent ranges
    }
}
