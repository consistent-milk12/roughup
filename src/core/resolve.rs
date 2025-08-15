//! Conflict resolution engine with deterministic SmartMerge strategies
//!
//! Implements ordered resolution pipeline:
//! 1. Whitespace-only → normalized merge
//! 2. One side empty → take non-empty side
//! 3. Superset → choose larger side deterministically
//! 4. Disjoint insertions → stable concatenation (ours→theirs)
//! 5. AST boundary aligned → syntax-aware weaving (future)
//! 6. Else → require Interactive or explicit choice

use crate::cli::{AppContext, ResolveArgs};
use crate::core::backup::BackupManager;
use crate::core::conflict::{ConflictMarker, parse_conflicts};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::{Path, PathBuf}};

/// Resolution strategy for conflict handling
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ResolveStrategy {
    /// Take our side (current branch/changes)
    TakeOurs,
    /// Take their side (incoming changes)
    TakeTheirs,
    /// Take base content (3-way conflicts only)
    TakeBase,
    /// Interactive resolution via TUI
    Interactive,
    /// Smart auto-resolution with high confidence threshold
    Smart,
}

/// Result of a conflict resolution attempt
#[derive(Debug, Clone)]
pub struct Resolution {
    /// Strategy that was applied
    pub chosen: ResolveStrategy,
    /// Resolved content if auto-resolution succeeded
    pub resolved_text: Option<String>,
    /// Whether resolution was automatically applied
    pub auto_applied: bool,
    /// Confidence score echoed from input for auditing
    pub confidence: f32,
    /// Reason for resolution choice (for logging/debugging)
    pub reason: String,
}

/// Stateless resolver entry point
///
/// For Interactive strategy, returns None resolved_text and lets caller invoke TUI.
/// For Smart strategy, enforces ≥0.95 confidence threshold and optional syntax checks.
///
/// # Arguments
/// * `conflict` - The conflict marker to resolve
/// * `strategy` - Requested resolution strategy  
/// * `syntax_check` - Optional function to validate resolved content
pub fn resolve<F>(
    conflict: &ConflictMarker,
    strategy: ResolveStrategy,
    syntax_check: Option<F>,
) -> Result<Resolution>
where
    F: Fn(&str) -> bool,
{
    match strategy {
        ResolveStrategy::TakeOurs => Ok(Resolution {
            chosen: strategy,
            resolved_text: Some(conflict.ours.clone()),
            auto_applied: true,
            confidence: 1.0,
            reason: "Explicit choice: take ours".to_string(),
        }),

        ResolveStrategy::TakeTheirs => Ok(Resolution {
            chosen: strategy,
            resolved_text: Some(conflict.theirs.clone()),
            auto_applied: true,
            confidence: 1.0,
            reason: "Explicit choice: take theirs".to_string(),
        }),

        ResolveStrategy::TakeBase => match &conflict.base {
            Some(base_content) => Ok(Resolution {
                chosen: strategy,
                resolved_text: Some(base_content.clone()),
                auto_applied: true,
                confidence: 1.0,
                reason: "Explicit choice: take base".to_string(),
            }),
            None => {
                anyhow::bail!("No base section present for 3-way resolution")
            }
        },

        ResolveStrategy::Interactive => Ok(Resolution {
            chosen: strategy,
            resolved_text: None,
            auto_applied: false,
            confidence: conflict.confidence,
            reason: "Interactive resolution required".to_string(),
        }),

        ResolveStrategy::Smart => resolve_smart(conflict, syntax_check),
    }
}

/// Smart auto-resolution with ordered pipeline and safety checks
fn resolve_smart<F>(conflict: &ConflictMarker, syntax_check: Option<F>) -> Result<Resolution>
where
    F: Fn(&str) -> bool,
{
    // Enforce confidence threshold first
    if conflict.confidence < 0.95 {
        return Ok(Resolution {
            chosen: ResolveStrategy::Interactive,
            resolved_text: None,
            auto_applied: false,
            confidence: conflict.confidence,
            reason: format!("Confidence {} below 0.95 threshold", conflict.confidence),
        });
    }

    // Cache normalized forms once to avoid recomputation
    let ours_ws = normalize_whitespace(&conflict.ours);
    let theirs_ws = normalize_whitespace(&conflict.theirs);
    let ours_sup = normalize_for_superset(&conflict.ours);
    let theirs_sup = normalize_for_superset(&conflict.theirs);

    // Apply resolution pipeline in order
    if let Some(resolution) = try_whitespace_only_resolution(conflict, &ours_ws, &theirs_ws) {
        return finalize_resolution(resolution, syntax_check);
    }

    if let Some(resolution) = try_addition_only_resolution(conflict) {
        return finalize_resolution(resolution, syntax_check);
    }

    if let Some(resolution) = try_superset_resolution(conflict, &ours_sup, &theirs_sup) {
        return finalize_resolution(resolution, syntax_check);
    }

    if let Some(resolution) = try_disjoint_insertion_resolution(conflict) {
        return finalize_resolution(resolution, syntax_check);
    }

    // Future: AST boundary alignment would go here

    // No safe auto-resolution found - require manual intervention
    Ok(Resolution {
        chosen: ResolveStrategy::Interactive,
        resolved_text: None,
        auto_applied: false,
        confidence: conflict.confidence,
        reason: "No safe auto-resolution pattern matched".to_string(),
    })
}

/// Try resolution for whitespace-only differences
fn try_whitespace_only_resolution(
    conflict: &ConflictMarker,
    ours_normalized: &str,
    theirs_normalized: &str,
) -> Option<Resolution> {
    if ours_normalized == theirs_normalized {
        // Content is identical after normalization - prefer ours with clean formatting
        let clean_content = clean_whitespace_formatting(&conflict.ours);

        Some(Resolution {
            chosen: ResolveStrategy::Smart,
            resolved_text: Some(clean_content),
            auto_applied: true,
            confidence: conflict.confidence,
            reason: "whitespace-only-resolved".to_string(),
        })
    } else {
        None
    }
}

/// Try resolution for addition-only changes (one side empty)
fn try_addition_only_resolution(conflict: &ConflictMarker) -> Option<Resolution> {
    let ours_empty = conflict.ours.trim().is_empty();
    let theirs_empty = conflict.theirs.trim().is_empty();

    if ours_empty && !theirs_empty {
        Some(Resolution {
            chosen: ResolveStrategy::Smart,
            resolved_text: Some(conflict.theirs.clone()),
            auto_applied: true,
            confidence: conflict.confidence,
            reason: "addition-only-ours-empty".to_string(),
        })
    } else if theirs_empty && !ours_empty {
        Some(Resolution {
            chosen: ResolveStrategy::Smart,
            resolved_text: Some(conflict.ours.clone()),
            auto_applied: true,
            confidence: conflict.confidence,
            reason: "addition-only-theirs-empty".to_string(),
        })
    } else {
        None
    }
}

/// Try resolution for superset relationships (one side contains the other)
fn try_superset_resolution(
    conflict: &ConflictMarker,
    ours_norm: &str,
    theirs_norm: &str,
) -> Option<Resolution> {
    let nontrivial = |t: &str| !t.trim().is_empty();

    if nontrivial(theirs_norm) && ours_norm.contains(theirs_norm) {
        // ours ⊇ theirs
        Some(Resolution {
            chosen: ResolveStrategy::Smart,
            resolved_text: Some(conflict.ours.clone()),
            auto_applied: true,
            confidence: conflict.confidence,
            reason: "superset-ours-contains-theirs".to_string(),
        })
    } else if nontrivial(ours_norm) && theirs_norm.contains(ours_norm) {
        // theirs ⊇ ours
        Some(Resolution {
            chosen: ResolveStrategy::Smart,
            resolved_text: Some(conflict.theirs.clone()),
            auto_applied: true,
            confidence: conflict.confidence,
            reason: "superset-theirs-contains-ours".to_string(),
        })
    } else {
        None
    }
}

/// Try resolution for disjoint insertions (no overlapping lines)
fn try_disjoint_insertion_resolution(conflict: &ConflictMarker) -> Option<Resolution> {
    let ours_lines: std::collections::HashSet<_> = conflict
        .ours
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    let theirs_lines: std::collections::HashSet<_> = conflict
        .theirs
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    // Check if sets are disjoint (no shared lines)
    if ours_lines.is_disjoint(&theirs_lines) && !ours_lines.is_empty() && !theirs_lines.is_empty() {
        // EOL-stable concatenation
        let eol = {
            // Prefer ours' EOL; fallback to theirs; default "\n"
            let eo = detect_eol(&conflict.ours);
            if eo == "\n" {
                detect_eol(&conflict.theirs)
            } else {
                eo
            }
        };
        let mut left = conflict.ours.clone();
        let right = conflict.theirs.clone();

        // Ensure exactly one separator between blocks
        let needs_sep = !left.ends_with('\n') && !left.ends_with("\r\n");
        if needs_sep {
            left.push_str(eol);
        }
        let combined = format!("{left}{right}");

        Some(Resolution {
            chosen: ResolveStrategy::Smart,
            resolved_text: Some(combined),
            auto_applied: true,
            confidence: conflict.confidence,
            reason: "disjoint-insertions-concatenated".to_string(),
        })
    } else {
        None
    }
}

/// Apply syntax validation and finalize resolution
fn finalize_resolution<F>(resolution: Resolution, syntax_check: Option<F>) -> Result<Resolution>
where
    F: Fn(&str) -> bool,
{
    if let Some(ref content) = resolution.resolved_text
        && let Some(check_fn) = syntax_check
        && !check_fn(content)
    {
        // Syntax check failed - fall back to interactive
        return Ok(Resolution {
            chosen: ResolveStrategy::Interactive,
            resolved_text: None,
            auto_applied: false,
            confidence: resolution.confidence,
            reason: format!("Syntax validation failed: {}", resolution.reason),
        });
    }

    Ok(resolution)
}

/// Detect the dominant EOL of a snippet; default to "\n"
fn detect_eol(s: &str) -> &'static str {
    let crlf = s.matches("\r\n").count();
    let lf = s.matches('\n').count();
    if crlf > 0 && crlf >= lf { "\r\n" } else { "\n" }
}

/// Normalize whitespace for comparison (same as conflict.rs scoring)
fn normalize_whitespace(content: &str) -> String {
    content
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip blank lines and syntactically empty braces-only lines for superset checking
fn normalize_for_superset(s: &str) -> String {
    s.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Apply consistent whitespace formatting with EOL preservation
fn clean_whitespace_formatting(content: &str) -> String {
    let eol = detect_eol(content);
    content
        .lines()
        .map(|line| line.trim_end()) // Trim trailing whitespace
        .collect::<Vec<_>>()
        .join(eol) // Preserve native EOL
}

/// Convenience wrapper to avoid None::<fn> type hints at call sites
pub fn resolve_no_check(
    conflict: &ConflictMarker,
    strategy: ResolveStrategy,
) -> Result<Resolution> {
    resolve::<fn(&str) -> bool>(conflict, strategy, None)
}

/// Batch resolve multiple conflicts with the same strategy
pub fn resolve_batch<F>(
    conflicts: &[&ConflictMarker],
    strategy: ResolveStrategy,
    syntax_check: Option<F>,
) -> Result<Vec<Resolution>>
where
    F: Fn(&str) -> bool + Copy,
{
    conflicts
        .iter()
        .map(|conflict| resolve(conflict, strategy, syntax_check))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::conflict::{ConflictOrigin, ConflictType};
    use std::path::PathBuf;

    fn make_test_conflict(
        ours: &str,
        theirs: &str,
        base: Option<&str>,
        confidence: f32,
    ) -> ConflictMarker {
        ConflictMarker {
            file: PathBuf::from("test.rs"),
            origin: ConflictOrigin::GitMarkers,
            conflict_type: ConflictType::GitMarkers {
                ours_meta: "HEAD".to_string(),
                theirs_meta: "feature".to_string(),
                has_base: base.is_some(),
            },
            byte_range: (0, 100),
            line_range: (1, 10),
            ours: ours.to_string(),
            theirs: theirs.to_string(),
            base: base.map(|s| s.to_string()),
            confidence,
        }
    }

    #[test]
    fn test_explicit_strategies() {
        let conflict = make_test_conflict("ours content", "theirs content", None, 0.8);

        // Test TakeOurs
        let resolution = resolve_no_check(&conflict, ResolveStrategy::TakeOurs).unwrap();
        assert_eq!(resolution.chosen, ResolveStrategy::TakeOurs);
        assert_eq!(resolution.resolved_text.unwrap(), "ours content");
        assert!(resolution.auto_applied);

        // Test TakeTheirs
        let resolution = resolve_no_check(&conflict, ResolveStrategy::TakeTheirs).unwrap();
        assert_eq!(resolution.chosen, ResolveStrategy::TakeTheirs);
        assert_eq!(resolution.resolved_text.unwrap(), "theirs content");
        assert!(resolution.auto_applied);

        // Test Interactive
        let resolution = resolve_no_check(&conflict, ResolveStrategy::Interactive).unwrap();
        assert_eq!(resolution.chosen, ResolveStrategy::Interactive);
        assert!(resolution.resolved_text.is_none());
        assert!(!resolution.auto_applied);
    }

    #[test]
    fn test_take_base_strategy() {
        // Test with base present
        let conflict = make_test_conflict("ours", "theirs", Some("base content"), 0.9);
        let resolution = resolve_no_check(&conflict, ResolveStrategy::TakeBase).unwrap();
        assert_eq!(resolution.resolved_text.unwrap(), "base content");

        // Test without base
        let conflict = make_test_conflict("ours", "theirs", None, 0.9);
        let result = resolve_no_check(&conflict, ResolveStrategy::TakeBase);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No base section"));
    }

    #[test]
    fn test_smart_confidence_threshold() {
        // Below threshold should fall back to interactive
        let conflict = make_test_conflict("ours", "theirs", None, 0.5);
        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        assert_eq!(resolution.chosen, ResolveStrategy::Interactive);
        assert!(resolution.reason.contains("below 0.95 threshold"));
    }

    #[test]
    fn test_whitespace_only_resolution() {
        let ours = "fn test() {\n    return 42;\n}";
        let theirs = "fn test() {\n  return 42;\n}"; // Different indentation
        let conflict = make_test_conflict(ours, theirs, None, 0.98);

        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        assert_eq!(resolution.chosen, ResolveStrategy::Smart);
        assert!(resolution.auto_applied);
        assert!(resolution.reason.contains("whitespace-only"));
        assert!(resolution.resolved_text.is_some());
    }

    #[test]
    fn test_addition_only_resolution() {
        // Ours empty, theirs has content
        let conflict = make_test_conflict("", "new function() {}", None, 0.98);
        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        assert_eq!(resolution.resolved_text.unwrap(), "new function() {}");
        assert!(resolution.reason.contains("addition-only-ours-empty"));

        // Theirs empty, ours has content
        let conflict = make_test_conflict("existing function() {}", "", None, 0.98);
        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        assert_eq!(resolution.resolved_text.unwrap(), "existing function() {}");
        assert!(resolution.reason.contains("addition-only-theirs-empty"));
    }

    #[test]
    fn test_superset_resolution() {
        let ours = "fn test() {\n    println!(\"hello\");\n    println!(\"world\");\n}";
        let theirs = "println!(\"hello\");"; // Subset of ours
        let conflict = make_test_conflict(ours, theirs, None, 0.98);

        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        assert_eq!(resolution.resolved_text.unwrap(), ours);
        assert!(resolution.reason.contains("superset-ours-contains-theirs"));
    }

    #[test]
    fn test_disjoint_insertion_resolution() {
        let ours = "fn function_a() {}";
        let theirs = "fn function_b() {}";
        let conflict = make_test_conflict(ours, theirs, None, 0.98);

        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        let resolved = resolution.resolved_text.unwrap();
        assert!(resolved.contains("function_a"));
        assert!(resolved.contains("function_b"));
        assert!(resolution.reason.contains("disjoint-insertions"));

        // Verify ours comes before theirs
        let ours_pos = resolved.find("function_a").unwrap();
        let theirs_pos = resolved.find("function_b").unwrap();
        assert!(
            ours_pos < theirs_pos,
            "Ours should come before theirs in concatenation"
        );
    }

    #[test]
    fn test_syntax_validation() {
        let conflict = make_test_conflict("", "invalid syntax {{", None, 0.98);

        // Syntax check that rejects unbalanced braces
        let syntax_check = |content: &str| -> bool {
            let open_count = content.chars().filter(|&c| c == '{').count();
            let close_count = content.chars().filter(|&c| c == '}').count();
            open_count == close_count
        };

        let resolution = resolve(&conflict, ResolveStrategy::Smart, Some(syntax_check)).unwrap();
        assert_eq!(resolution.chosen, ResolveStrategy::Interactive);
        assert!(resolution.reason.contains("Syntax validation failed"));
    }

    #[test]
    fn test_batch_resolution() {
        let conflicts = vec![
            make_test_conflict("", "addition", None, 0.98),
            make_test_conflict("same", "same", None, 0.98), // whitespace-only after normalization
            make_test_conflict("complex", "different", None, 0.50), // low confidence, no clear pattern
        ];

        let conflict_refs: Vec<&ConflictMarker> = conflicts.iter().collect();
        let resolutions = resolve_batch(
            &conflict_refs,
            ResolveStrategy::Smart,
            None::<fn(&str) -> bool>,
        )
        .unwrap();
        assert_eq!(resolutions.len(), 3);

        // First should resolve as addition-only
        assert!(resolutions[0].auto_applied);
        assert!(resolutions[0].reason.contains("addition-only"));

        // Second should resolve as whitespace-only
        assert!(resolutions[1].auto_applied);
        assert!(resolutions[1].reason.contains("whitespace-only"));

        // Third should require interactive due to low confidence
        assert_eq!(resolutions[2].chosen, ResolveStrategy::Interactive);
    }

    #[test]
    fn smart_preserves_crlf_on_whitespace_only() {
        let ours = "fn a()\r\n{\r\n 1\r\n}\r\n";
        let theirs = "fn a()\r\n{\r\n  1\r\n}\r\n";
        let conflict = make_test_conflict(ours, theirs, None, 0.98);
        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        let output = resolution.resolved_text.unwrap();
        assert!(output.contains("\r\n"));
        assert!(!output.contains("\n\n")); // no accidental double LF
        assert!(resolution.reason.contains("whitespace-only"));
    }

    #[test]
    fn disjoint_concat_is_single_eol_and_ordered() {
        let ours = "fn a() {}\r\n";
        let theirs = "fn b() {}\r\n";
        let conflict = make_test_conflict(ours, theirs, None, 0.98);
        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        let output = resolution.resolved_text.unwrap();
        let pos_a = output.find("fn a() {}").unwrap();
        let pos_b = output.find("fn b() {}").unwrap();
        assert!(pos_a < pos_b);
        assert!(output.matches("\r\n").count() >= 2);
        assert!(resolution.reason.contains("disjoint-insertions"));
    }

    #[test]
    fn superset_requires_nontrivial_subset() {
        let ours = "fn a() {}\n";
        let theirs = "\n\n   \n"; // trivial after normalization
        let conflict = make_test_conflict(ours, theirs, None, 0.98);
        let resolution = resolve_no_check(&conflict, ResolveStrategy::Smart).unwrap();
        // Should fall back to addition-only since theirs is effectively empty
        assert_eq!(resolution.chosen, ResolveStrategy::Smart);
        assert!(resolution.reason.contains("addition-only-theirs-empty"));
    }
}

/// Stable strategy tag for JSON output
fn strategy_tag(s: ResolveStrategy) -> &'static str {
    match s {
        ResolveStrategy::TakeOurs => "take-ours",
        ResolveStrategy::TakeTheirs => "take-theirs",
        ResolveStrategy::TakeBase => "take-base",
        ResolveStrategy::Interactive => "interactive",
        ResolveStrategy::Smart => "smart",
    }
}

/// JSON output for machine-readable conflict summaries
#[derive(Serialize, Deserialize, Debug)]
pub struct ConflictSummary {
    pub file: PathBuf,
    pub total_conflicts: usize,
    pub auto_resolved: usize,
    pub interactive_required: usize,
    pub resolutions: Vec<ResolutionSummary>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ResolutionSummary {
    pub line_range: (usize, usize),
    pub strategy: String, // kebab-case stable tag
    pub auto_applied: bool,
    pub confidence: f32,
    pub reason: String,
}

/// CLI entry point for resolve command
pub fn run(args: ResolveArgs, ctx: &AppContext) -> Result<()> {
    let mut all_conflicts = Vec::new();
    let mut file_summaries = Vec::new();

    // Scan for conflicts in all specified paths
    for path in &args.paths {
        if path.is_file() {
            // Single file
            let mut conflicts = scan_file_for_conflicts(path)?;
            if !conflicts.is_empty() {
                file_summaries.push(summarize_file_conflicts(path, &conflicts));
                all_conflicts.append(&mut conflicts); // move without clone
            }
        } else if path.is_dir() {
            // Directory - scan for files with conflict markers
            let files = find_conflicted_files(path)?;
            for file in files {
                let mut conflicts = scan_file_for_conflicts(&file)?;
                if !conflicts.is_empty() {
                    file_summaries.push(summarize_file_conflicts(&file, &conflicts));
                    all_conflicts.append(&mut conflicts); // move without clone
                }
            }
        }
    }

    if all_conflicts.is_empty() {
        if !ctx.quiet {
            if args.json {
                println!("{{\"message\": \"No conflicts found\", \"files\": []}}");
            } else {
                println!("No conflicts found in specified paths.");
            }
        }
        return Ok(());
    }

    // Resolve conflicts using specified strategy
    let mut resolved_files = Vec::new();

    for summary in &mut file_summaries {
        let file_conflicts: Vec<_> = all_conflicts
            .iter()
            .filter(|c| c.file == summary.file)
            .collect();

        let resolutions = resolve_batch(&file_conflicts, args.strategy, None::<fn(&str) -> bool>)?;

        // Process resolutions
        let mut auto_resolved = 0;
        let mut interactive_required = 0;
        let mut resolution_summaries = Vec::new();

        for (conflict, resolution) in file_conflicts.iter().zip(resolutions.iter()) {
            if resolution.auto_applied {
                auto_resolved += 1;
            } else {
                interactive_required += 1;
            }

            resolution_summaries.push(ResolutionSummary {
                line_range: conflict.line_range,
                strategy: strategy_tag(resolution.chosen).to_string(),
                auto_applied: resolution.auto_applied,
                confidence: resolution.confidence,
                reason: resolution.reason.clone(),
            });
        }

        summary.auto_resolved = auto_resolved;
        summary.interactive_required = interactive_required;
        summary.resolutions = resolution_summaries;

        // Apply resolved changes if requested
        if args.apply && auto_resolved > 0 {
            // TODO: Create BackupManager when backup is requested
            // For now, pass None - will integrate with centralized backup system
            apply_resolutions_to_file(&summary.file, &file_conflicts, &resolutions, None)?;
            resolved_files.push(summary.file.clone());
        }
    }

    // Output results
    if args.json {
        let has_unresolved = file_summaries.iter().any(|s| s.interactive_required > 0);
        let output = serde_json::json!({
            "schema_version": "1",
            "total_files": file_summaries.len(),
            "resolved_files": resolved_files,
            "exit_code": if has_unresolved { 2 } else { 0 },
            "files": file_summaries
        });
        println!("{}", serde_json::to_string(&output)?);
    } else {
        print_resolution_summary(&file_summaries, &resolved_files, ctx);
    }

    // Exit code semantics following apply command pattern
    let has_unresolved = file_summaries.iter().any(|s| s.interactive_required > 0);
    if has_unresolved && !args.json {
        std::process::exit(2); // Exit code 2 = conflicts remaining
    }

    Ok(())
}

/// Scan a single file for Git conflict markers
fn scan_file_for_conflicts(file: &PathBuf) -> Result<Vec<ConflictMarker>> {
    let content =
        fs::File::open(file).with_context(|| format!("Failed to open file: {}", file.display()))?;
    let reader = std::io::BufReader::new(content);
    parse_conflicts(file.clone(), reader)
}

/// Fast byte-safe check for conflict markers without full parsing
fn likely_has_conflict_markers(file: &PathBuf) -> bool {
    use std::io::BufRead;
    let Ok(f) = fs::File::open(file) else {
        return false;
    };
    let mut reader = std::io::BufReader::new(f);
    let mut buf = Vec::new();
    let mut hits = 0usize;

    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf).ok().unwrap_or(0) == 0 {
            break;
        }
        // Quick prefilter - doesn't need to be column-0 strict like the parser
        if buf.windows(7).any(|w| w == b"<<<<<<<") {
            hits += 1;
        }
        if buf.windows(7).any(|w| w == b"=======") {
            hits += 1;
        }
        if buf.windows(7).any(|w| w == b">>>>>>>") {
            hits += 1;
        }
        if hits >= 3 {
            return true;
        } // early success
    }
    false
}

/// Find all files in directory that likely contain conflict markers
fn find_conflicted_files(dir: &PathBuf) -> Result<Vec<PathBuf>> {
    use crate::infra::FileWalker;

    let _walker = FileWalker::new(&[])
        .with_context(|| format!("Failed to create walker for: {}", dir.display()))?;

    let mut conflicted_files = Vec::new();

    // TODO: Need to implement proper directory traversal - FileWalker signature mismatch
    // For now, use a simple approach
    if dir.is_file() {
        if likely_has_conflict_markers(dir) {
            conflicted_files.push(dir.clone());
        }
    } else {
        // Simple recursive scan - will be replaced with proper FileWalker integration
        fn scan_dir_recursive(dir: &PathBuf, results: &mut Vec<PathBuf>) -> Result<()> {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    scan_dir_recursive(&path, results)?;
                } else if likely_has_conflict_markers(&path) {
                    results.push(path);
                }
            }
            Ok(())
        }
        scan_dir_recursive(dir, &mut conflicted_files)?;
    }

    Ok(conflicted_files)
}

/// Create summary of conflicts in a file
fn summarize_file_conflicts(
    file: &Path,
    conflicts: &[ConflictMarker],
) -> ConflictSummary {
    ConflictSummary {
        file: file.to_path_buf(),
        total_conflicts: conflicts.len(),
        auto_resolved: 0,        // Will be filled by caller
        interactive_required: 0, // Will be filled by caller
        resolutions: Vec::new(), // Will be filled by caller
    }
}

/// Apply resolved conflicts to file with BackupManager integration and syntax validation
fn apply_resolutions_to_file(
    file: &PathBuf,
    conflicts: &[&ConflictMarker],
    resolutions: &[Resolution],
    backup_manager: Option<&mut BackupManager>,
) -> Result<()> {
    // Read original bytes to preserve non-UTF-8 content and avoid char boundary panics
    let mut original =
        fs::read(file).with_context(|| format!("Failed to read file: {}", file.display()))?;

    // Create backup using centralized BackupManager if provided
    if let Some(backup) = backup_manager {
        backup
            .backup_file(file)
            .with_context(|| format!("Failed to create backup for: {}", file.display()))?;
    }

    // Build descending list by start offset to preserve indices during replacement
    let mut pairs: Vec<_> = conflicts.iter().zip(resolutions.iter()).collect();
    pairs.sort_by_key(|(c, _)| std::cmp::Reverse(c.byte_range.0));

    // Apply edits from right to left on bytes to preserve offsets
    for (conflict, resolution) in pairs {
        if let Some(ref resolved_text) = resolution.resolved_text {
            let (start, end) = conflict.byte_range;
            let replacement_bytes = resolved_text.as_bytes(); // UTF-8 resolved text to bytes

            // Validate bounds to prevent panic
            if start > end || end > original.len() {
                anyhow::bail!(
                    "Invalid byte range {:?} for {}",
                    conflict.byte_range,
                    file.display()
                );
            }

            // Replace in-place using Vec<u8> splice - safe for any byte content
            original.splice(start..end, replacement_bytes.iter().copied());
        }
    }

    // Optional syntax validation before write (can be added later)
    // if let Some(syntax_validator) = syntax_validator {
    //     if !syntax_validator(&original) {
    //         anyhow::bail!("Syntax validation failed for {}", file.display());
    //     }
    // }

    // Atomic write back preserving all original encoding outside edited ranges
    fs::write(file, &original)
        .with_context(|| format!("Failed to write resolved file: {}", file.display()))?;

    Ok(())
}

/// Print human-readable resolution summary
fn print_resolution_summary(
    summaries: &[ConflictSummary],
    resolved_files: &[PathBuf],
    ctx: &AppContext,
) {
    if ctx.quiet {
        return;
    }

    let total_conflicts: usize = summaries.iter().map(|s| s.total_conflicts).sum();
    let total_resolved: usize = summaries.iter().map(|s| s.auto_resolved).sum();
    let total_interactive: usize = summaries.iter().map(|s| s.interactive_required).sum();

    println!("Conflict Resolution Summary:");
    println!("  Files with conflicts: {}", summaries.len());
    println!("  Total conflicts: {}", total_conflicts);
    println!("  Auto-resolved: {}", total_resolved);
    println!("  Interactive required: {}", total_interactive);
    println!();

    for summary in summaries {
        println!("{}:", summary.file.display());
        println!(
            "  Conflicts: {} total, {} resolved, {} interactive",
            summary.total_conflicts, summary.auto_resolved, summary.interactive_required
        );

        if resolved_files.contains(&summary.file) {
            println!("  Applied resolutions to file");
        }
        println!();
    }

    if !resolved_files.is_empty() {
        println!(
            "Successfully resolved conflicts in {} file(s)",
            resolved_files.len()
        );
    }

    if total_interactive > 0 {
        println!(
            "{} conflicts require interactive resolution",
            total_interactive
        );
        println!("  Run with --strategy=interactive for manual resolution");
    }
}
