//! CLI command handlers for anchor detection and hints.
//!
//! Provides `--hint-anchors` and `--why file:line` functionality with
//! rich error reporting using miette and ariadne.

use std::fs;

use anyhow::Result;
use ariadne::{Color, Label, Report, ReportKind, Source};
use camino::{Utf8Path, Utf8PathBuf};
use clap::Args;
use owo_colors::OwoColorize;
use serde_json::json;
use tabled::{Table, Tabled};
use tracing::{info, instrument};

use crate::{anchor::detect::{AnchorHints, BadAnchorError, FnHit, hint_anchors}, cli::AppContext};

/// Anchor validation and hint arguments.
#[derive(Debug, Clone, Args)]
pub struct AnchorArgs
{
    /// Enable anchor hints and validation
    #[arg(long)]
    pub hint_anchors: bool,

    /// Explain why a specific line was included/excluded
    #[arg(long, value_name = "FILE:LINE")]
    pub why: Option<String>,

    /// Output format
    #[arg(long, default_value = "text", value_enum)]
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OutputFormat
{
    Text,
    Json,
    Table,
}

/// Validate an anchor and provide hints if invalid.
#[instrument(skip(root))]
pub fn validate_anchor_with_hints(
    root: &Utf8Path,
    anchor: &str,
    args: &AnchorArgs,
) -> Result<Option<FnHit>>
{
    let (file, line) = parse_anchor(anchor)?;
    let hints = hint_anchors(root, &file, line)?;

    match hints
    {
        AnchorHints::Good { function } =>
        {
            if args.hint_anchors
            {
                print_good_anchor(&function, line, args);
            }

            Ok(Some(function))
        }

        AnchorHints::OffByN { requested_line, actual, offset } =>
        {
            if args.hint_anchors
            {
                print_off_by_n_hint(&file, requested_line, &actual, offset, args)?;
            }

            // Return the actual function, but warn
            info!("Anchor is {} lines off from function start", offset);

            Ok(Some(actual))
        }

        AnchorHints::OutsideScope { requested_line, nearest } =>
        {
            if args.hint_anchors || !nearest.is_empty()
            {
                print_outside_scope_hint(&file, requested_line, &nearest, args)?;
            }

            // Return error with suggestions
            let error = create_bad_anchor_error(&file, requested_line, &nearest)?;
            Err(error.into())
        }

        AnchorHints::NotAFile { path, reason } =>
        {
            if args.hint_anchors
            {
                print_not_a_file(&path, &reason, args);
            }

            anyhow::bail!("Invalid anchor: {} - {}", path, reason)
        }
    }
}

/// Parse "file:line" anchor format.
fn parse_anchor(anchor: &str) -> Result<(Utf8PathBuf, usize)>
{
    let parts: Vec<&str> = anchor
        .rsplitn(2, ':')
        .collect();
    if parts.len() != 2
    {
        anyhow::bail!("Invalid anchor format: expected 'file:line'");
    }

    let line = parts[0]
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("Invalid line number: {}", parts[0]))?;
    let file = Utf8PathBuf::from(parts[1]);

    Ok((file, line))
}

/// Print confirmation for a good anchor.
fn print_good_anchor(
    func: &FnHit,
    current_line: usize,
    args: &AnchorArgs,
)
{
    match args.format
    {
        OutputFormat::Json =>
        {
            let output = json!({
                "status": "valid",
                "function": func,
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        OutputFormat::Table =>
        {
            #[derive(Tabled)]
            struct AnchorInfo
            {
                status: String,
                function: String,
                location: String,
                kind: String,
            }

            let info = AnchorInfo {
                status: "✓ Valid"
                    .green()
                    .to_string(),
                function: func
                    .qualified_name
                    .clone(),
                location: format!("{}:{}-{}", func.file, func.start_line, func.end_line),
                kind: format!("{:?}", func.kind),
            };

            let table = Table::new(vec![info]).to_string();
            println!("{}", table);
        }
        OutputFormat::Text =>
        {
            let offset = current_line as isize - func.start_line as isize;
            let offset_hint = if offset == 0 {
                String::new() // At function start, no offset needed
            } else {
                format!(", +{} from start", offset)
            };
            
            println!(
                "{} {} (inside {} [{}..{}]{})",
                "Good".green().bold(),
                "Anchor is valid".bright_white(),
                func.name.cyan(),
                func.start_line,
                func.end_line,
                offset_hint
            );
            println!(
                "  Function: {}",
                func.qualified_name
                    .cyan()
            );
            println!(
                "  Location: {}:{}-{}",
                func.file, func.start_line, func.end_line
            );
        }
    }
}

/// Print hint for off-by-N anchor.
fn print_off_by_n_hint(
    file: &Utf8Path,
    requested: usize,
    actual: &FnHit,
    offset: isize,
    args: &AnchorArgs,
) -> Result<()>
{
    match args.format
    {
        OutputFormat::Json =>
        {
            let output = json!({
                "status": "off_by_n",
                "requested_line": requested,
                "actual_function": actual,
                "offset": offset,
                "suggestion": format!("{}:{}", file, actual.start_line),
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text =>
        {
            // Load file for snippet display
            let source = fs::read_to_string(file)?;
            let source_id = file.to_string();

            // Create ariadne report
            let offset_desc = if offset > 0
            {
                format!("{} lines inside function", offset)
            }
            else
            {
                format!("{} lines before function", -offset)
            };

            Report::build(ReportKind::Warning, (&source_id, requested..requested))
                .with_message("Anchor points inside function body")
                .with_label(
                    Label::new((&source_id, requested..requested))
                        .with_message(format!("anchor is {}", offset_desc))
                        .with_color(Color::Yellow),
                )
                .with_label(
                    Label::new((&source_id, actual.start_line..actual.start_line))
                        .with_message("function starts here")
                        .with_color(Color::Green),
                )
                .with_help(format!(
                    "Consider using '{}:{}' to anchor at function start",
                    file, actual.start_line
                ))
                .finish()
                .print((&source_id, Source::from(source)))
                .map_err(|e| anyhow::anyhow!("Failed to print report: {}", e))?;
        }
        _ => print_good_anchor(actual, requested, args), // Fallback to showing the function
    }

    Ok(())
}

/// Print hint for anchor outside any function scope.
fn print_outside_scope_hint(
    file: &Utf8Path,
    requested: usize,
    nearest: &[FnHit],
    args: &AnchorArgs,
) -> Result<()>
{
    match args.format
    {
        OutputFormat::Json =>
        {
            let suggestions: Vec<_> = nearest
                .iter()
                .map(|f| {
                    json!({
                        "function": &f.qualified_name,
                        "location": format!("{}:{}", f.file, f.start_line),
                        "distance": (f.start_line as isize - requested as isize).abs(),
                    })
                })
                .collect();

            let output = json!({
                "status": "outside_scope",
                "requested_line": requested,
                "suggestions": suggestions,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Table =>
        {
            #[derive(Tabled)]
            struct Suggestion
            {
                function: String,
                location: String,
                distance: usize,
            }

            let suggestions: Vec<_> = nearest
                .iter()
                .map(|f| {
                    Suggestion {
                        function: f
                            .qualified_name
                            .clone(),
                        location: format!("{}:{}", f.file, f.start_line),
                        distance: (f.start_line as isize - requested as isize).unsigned_abs(),
                    }
                })
                .collect();

            println!(
                "{} No function at line {} (outside scope)",
                "OutsideScope".red().bold(),
                requested
            );
            println!("\nNearest functions:");
            let table = Table::new(suggestions).to_string();
            println!("{}", table);
        }
        OutputFormat::Text =>
        {
            let source = fs::read_to_string(file)?;
            let source_id = file.to_string();

            let mut report = Report::build(ReportKind::Error, (&source_id, requested..requested))
                .with_message("No function found at this line")
                .with_label(
                    Label::new((&source_id, requested..requested))
                        .with_message("anchor points here")
                        .with_color(Color::Red),
                );

            // Add labels for nearest functions
            for (i, func) in nearest
                .iter()
                .take(3)
                .enumerate()
            {
                let color = match i
                {
                    0 => Color::Green,
                    1 => Color::Yellow,
                    _ => Color::Blue,
                };

                report = report.with_label(
                    Label::new((&source_id, func.start_line..func.start_line))
                        .with_message(format!("suggestion #{}: {}", i + 1, func.name))
                        .with_color(color),
                );
            }

            let suggestions_text = nearest
                .iter()
                .take(3)
                .map(|f| format!("  • {}:{} ({})", f.file, f.start_line, f.name))
                .collect::<Vec<_>>()
                .join("\n");

            report = report.with_help(format!("Try one of these anchors:\n{}", suggestions_text));

            report
                .finish()
                .print((&source_id, Source::from(source)))
                .map_err(|e| anyhow::anyhow!("Failed to print report: {}", e))?;
        }
    }

    Ok(())
}

/// Print error for non-file paths.
fn print_not_a_file(
    path: &Utf8Path,
    reason: &str,
    args: &AnchorArgs,
)
{
    match args.format
    {
        OutputFormat::Json =>
        {
            let output = json!({
                "status": "not_a_file",
                "path": path,
                "reason": reason,
            });
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
        }
        _ =>
        {
            eprintln!(
                "{} Invalid anchor path: {}",
                "✗"
                    .red()
                    .bold(),
                path
            );
            eprintln!("  Reason: {}", reason);
        }
    }
}

/// Create a detailed error for bad anchors.
fn create_bad_anchor_error(
    file: &Utf8Path,
    line: usize,
    nearest: &[FnHit],
) -> Result<BadAnchorError>
{
    let source = fs::read_to_string(file)?;

    let suggestions: Vec<(usize, String)> = nearest
        .iter()
        .take(3)
        .map(|f| (f.start_line, format!("{} ({:?})", f.name, f.kind)))
        .collect();

    let help_text = if suggestions.is_empty()
    {
        "No functions found in this file".to_string()
    }
    else
    {
        format!(
            "Try anchoring to one of these functions:\n{}",
            suggestions
                .iter()
                .map(|(line, desc)| format!("  • Line {}: {}", line, desc))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    // Calculate byte offset for the error span
    let line_start = source
        .lines()
        .take(line - 1)
        .map(|l| l.len() + 1)
        .sum::<usize>();

    Ok(BadAnchorError {
        file: file.to_path_buf(),
        line,
        src: source,
        anchor_span: (line_start, 1).into(),
        help: help_text,
        suggestions,
    })
}

/// Handle the `--why file:line` query to explain inclusion/exclusion.
#[instrument(skip(root))]
pub fn explain_why(
    root: &Utf8Path,
    query: &str,
    args: &AnchorArgs,
) -> Result<()>
{
    let (file, line) = parse_anchor(query)?;
    let hints = hint_anchors(root, &file, line)?;

    let explanation = match hints
    {
        AnchorHints::Good { function } =>
        {
            let offset = line as isize - function.start_line as isize;
            let mut result = json!({
                "schema_version": 1,
                "query": query,
                "status": "Good",
                "reason": "Line is inside a function",
                "requested_line": line,
                "function": function, // Use the function directly to match test expectations
                "factors": {
                    "anchor_validity": "perfect",
                    "structural_importance": "high",
                    "likely_relevance": 0.95,
                }
            });
            
            // Include offset for non-start lines to aid tooling
            if offset != 0 {
                result["offset"] = json!(offset);
            }
            
            result
        }
        AnchorHints::OffByN { requested_line, actual, offset } =>
        {
            json!({
                "schema_version": 1,
                "query": query,
                "status": "OffByN",
                "reason": format!("Line is {} lines from function start", offset.abs()),
                "requested_line": requested_line,
                "offset": offset,
                "function": {
                    "name": actual.name,
                    "qualified_name": actual.qualified_name,
                    "file": actual.file,
                    "start_line": actual.start_line,
                    "end_line": actual.end_line,
                    "kind": actual.kind,
                    "confidence": actual.confidence
                },
                "factors": {
                    "anchor_validity": "near_miss",
                    "structural_importance": "medium",
                    "likely_relevance": 0.7,
                }
            })
        }
        AnchorHints::OutsideScope { requested_line, nearest } =>
        {
            json!({
                "schema_version": 1,
                "query": query,
                "status": "OutsideScope",
                "reason": "Line is not within any function scope",
                "requested_line": requested_line,
                "function": null, // Add explicit null function field for consistency
                "nearest_functions": nearest, // Keep field name matching test expectations
                "factors": {
                    "anchor_validity": "invalid",
                    "structural_importance": "low",
                    "likely_relevance": 0.2,
                }
            })
        }
        AnchorHints::NotAFile { path, reason } =>
        {
            json!({
                "schema_version": 1,
                "query": query,
                "status": "NotAFile",
                "reason": reason,
                "path": path,
            })
        }
    };

    match args.format
    {
        OutputFormat::Json =>
        {
            println!("{}", serde_json::to_string_pretty(&explanation)?);
        }
        _ =>
        {
            // Pretty print explanation
            println!(
                "{}",
                "═"
                    .repeat(60)
                    .bright_black()
            );
            println!("{} {}", "Query:".bold(), query);
            println!("{} {}", "Status:".bold(), explanation["status"]);
            println!("{} {}", "Reason:".bold(), explanation["reason"]);

            if let Some(factors) = explanation.get("factors")
            {
                println!("\n{}:", "Scoring Factors".bold());
                if let Some(obj) = factors.as_object()
                {
                    for (key, value) in obj
                    {
                        println!("  • {}: {}", key.replace('_', " "), value);
                    }
                }
            }

            println!(
                "{}",
                "═"
                    .repeat(60)
                    .bright_black()
            );
        }
    }

    Ok(())
}

/// Main entry point for anchor command handling.
#[instrument(skip(_ctx))]
pub fn run_anchor_command(
    args: &AnchorArgs,
    _ctx: &AppContext,
) -> Result<()>
{
    // Handle --why flag for explanation
    if let Some(query) = &args.why
    {
        let root = Utf8Path::new(".");
        return explain_why(root, query, args);
    }

    // If --hint-anchors is specified but no specific anchor provided, show usage
    if args.hint_anchors
    {
        eprintln!("Usage: rup anchor --hint-anchors with --why FILE:LINE");
        eprintln!("  or: rup context --anchor FILE:LINE --hint-anchors [queries...]");
        eprintln!();
        eprintln!("Examples:");
        eprintln!("  rup anchor --why src/main.rs:42");
        eprintln!("  rup context --anchor src/lib.rs:100 --hint-anchors 'pub fn'");
        return Ok(());
    }

    // Default: show help
    eprintln!("Anchor validation and positioning tool");
    eprintln!();
    eprintln!("Use --why FILE:LINE to analyze anchor placement");
    eprintln!("Use --hint-anchors with context command for validation");

    Ok(())
}
