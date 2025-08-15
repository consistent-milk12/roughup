//! Local scoreboard for `rup context` quality metrics.
//! Produces one JSON line per scenario in `scoreboard.jsonl`.
//!
//! Metrics:
//!   - cef: baseline_tokens / actual_tokens
//!   - dcr: 1 - (unique_item_ids / total_item_ids)
//!   - pfr: probe_first ? 1.0 : 0.0
//!   - deterministic_json_equal: true if repeated runs match bytes
//!
//! Baseline tokens:
//!   - Union of whole files referenced by the final JSON items, tokenized with the
//!     requested encoding.
//!
//! Usage:
//!   cargo run --bin scoreboard -- \
//!     --plan fixtures/fixture_plan.example.json \
//!     --out scoreboard.jsonl
//!
//! Design notes:
//!   - Local-only; no network.
//!   - Fails fast on plan errors and missing dependencies.
//!   - Compares identical runs under the same environment for determinism.
//!   - Path deduplication handles Windows-style paths (C:\...) correctly.

use std::collections::HashSet; // path deduplication
use std::fs; // read/write files
use std::io::Write; // write JSONL lines
use std::path::{Path, PathBuf}; // paths
use std::process::Command; // run `rup`

use serde::{Deserialize, Serialize}; // JSON serde
use tiktoken_rs::{CoreBPE, get_bpe_from_tokenizer, tokenizer::Tokenizer}; // tokenization

// --------------------------
// Plan file data structures
// --------------------------

#[derive(Deserialize)]
struct Plan
{
    // Tokenizer/encoding id (e.g., "o200k_base")
    encoding: String,
    // List of independent scenarios to execute
    scenarios: Vec<Scenario>,
}

#[derive(Deserialize)]
struct Scenario
{
    // Unique scenario name for reporting
    name: String,

    // Path to the fixture root (repo root)
    fixture_path: String,

    // One or more query strings for `rup context`
    queries: Vec<String>,

    // Optional tier ("A" | "B" | "C")
    tier: Option<String>,

    // Optional explicit budget (overrides tier)
    budget: Option<usize>,

    // Whether this scenario should count toward PFR
    probe_first: bool,

    // One or more runs; if two, we check determinism
    runs: Vec<Run>,
}

#[expect(unused, reason = "TODO: MARKED FOR USE")]
#[derive(Deserialize)]
struct Run
{
    // Extra args to pass to `rup context`
    args: Vec<String>,
    // Label for bookkeeping (e.g., "first", "second")
    label: String,
}

// --------------------------
// Output record per scenario
// --------------------------

#[derive(Serialize)]
struct ScoreRow
{
    // Scenario identity
    name: String,
    // Encoding actually used
    encoding: String,
    // Aggregate metrics
    cef: f64,
    dcr: f64,
    pfr: f64,
    deterministic_json_equal: bool,
    // Token tallies
    actual_tokens: usize,
    baseline_tokens: usize,
    // Additional useful metrics
    within_budget: bool,
    items_count: usize,
    // Optional helpful echoes
    tier: Option<String>,
    budget: Option<usize>,
}

// --------------------------
// Utility: run a command
// --------------------------

fn run_cmd_in(
    dir: &Path,
    prog: &str,
    args: &[&str],
) -> anyhow::Result<Vec<u8>>
{
    // Spawn child process in `dir`
    let out = Command::new(prog)
        .current_dir(dir)
        .args(args)
        .output()?;
    // Surface non-zero exit codes
    if !out
        .status
        .success()
    {
        anyhow::bail!(
            "{} failed: status={:?}\nstdout={}\nstderr={}",
            prog,
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    // Return raw stdout bytes
    Ok(out.stdout)
}

// --------------------------
// Tokenization helpers
// --------------------------

fn bpe(encoding: &str) -> anyhow::Result<CoreBPE>
{
    // Acquire a tokenizer by name (local, no network)
    match encoding
    {
        "o200k_base" => Ok(get_bpe_from_tokenizer(Tokenizer::O200kBase)?),
        "cl100k_base" => Ok(get_bpe_from_tokenizer(Tokenizer::Cl100kBase)?),
        "p50k_base" => Ok(get_bpe_from_tokenizer(Tokenizer::P50kBase)?),
        _ => anyhow::bail!("Unsupported encoding: {}", encoding),
    }
}

fn count_tokens(
    bpe: &CoreBPE,
    s: &str,
) -> usize
{
    // Count tokens deterministically
    bpe.encode_with_special_tokens(s)
        .len()
}

// --------------------------
// Baseline strategy
// --------------------------
//
// Build the union of whole files that appear in the final
// `rup context --json` items[].id. Each id has the form:
//   "<path>:<start>-<end>"
// We read each distinct <path> relative to the fixture root,
// concatenate contents, and tokenize.

#[expect(unused, reason = "TODO: MARKED FOR USE")]
#[derive(Deserialize)]
struct JsonItem
{
    id: String,
    tokens: usize,
    // content omitted intentionally to avoid allocation - we only need id and tokens
}

#[expect(unused, reason = "TODO: MARKED FOR USE")]
#[derive(Deserialize)]
struct JsonOut
{
    model: String,
    budget: usize,
    total_tokens: usize,
    items: Vec<JsonItem>,
    // Optional extras (ignored if absent)
    tier: Option<String>,
    effective_limit: Option<usize>,
    effective_top_per_query: Option<usize>,
}

fn parse_json_output(bytes: &[u8]) -> anyhow::Result<JsonOut>
{
    // Parse single-line JSON from `rup context`
    let v: JsonOut = serde_json::from_slice(bytes)?;
    Ok(v)
}

fn extract_paths(items: &[JsonItem]) -> Vec<String>
{
    // Collect file paths from item ids before the LAST colon (handles Windows C:\... paths)
    // Using HashSet ensures proper deduplication even for non-adjacent duplicates
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for it in items
    {
        if let Some((p, _rest)) = it
            .id
            .rsplit_once(':')
            && seen.insert(p)
        {
            out.push(p.to_string());
        }
    }
    out
}

fn read_whole_files(
    root: &Path,
    rels: &[String],
) -> anyhow::Result<String>
{
    // Concatenate each file's entire content with a newline
    let mut out = String::new();
    for r in rels
    {
        let p = root.join(r);
        if p.exists()
        {
            let s = fs::read_to_string(&p)?;
            out.push_str(&s);
            out.push('\n');
        }
    }
    Ok(out)
}

// --------------------------
// Metric computations
// --------------------------

fn dcr_from_items(items: &[JsonItem]) -> f64
{
    // DCR = 1 - unique_ids / total_ids
    if items.is_empty()
    {
        return 0.0;
    }
    let total = items.len() as f64;
    let mut ids = items
        .iter()
        .map(|i| {
            i.id.clone()
        })
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    let unique = ids.len() as f64;
    1.0 - (unique / total)
}

fn cef(
    baseline_tokens: usize,
    actual_tokens: usize,
) -> f64
{
    // Avoid division by zero; clamp to 0.0
    if actual_tokens == 0
    {
        return 0.0;
    }
    (baseline_tokens as f64) / (actual_tokens as f64)
}

// --------------------------
// Main: plan → JSONL
// --------------------------

fn main() -> anyhow::Result<()>
{
    // Read args: --plan <file> --out <file>
    let args = std::env::args()
        .skip(1)
        .collect::<Vec<_>>();
    // Simple manual parsing to avoid bringing in Clap here
    let plan_idx = args
        .iter()
        .position(|a| a == "--plan")
        .ok_or_else(|| anyhow::anyhow!("missing --plan"))?;
    let out_idx = args
        .iter()
        .position(|a| a == "--out")
        .ok_or_else(|| anyhow::anyhow!("missing --out"))?;
    let plan_path = args
        .get(plan_idx + 1)
        .ok_or_else(|| anyhow::anyhow!("--plan needs a value"))?;
    let out_path = args
        .get(out_idx + 1)
        .ok_or_else(|| anyhow::anyhow!("--out needs a value"))?;

    // Load and parse the plan file
    let plan_bytes = fs::read(plan_path)?;
    let plan: Plan = serde_json::from_slice(&plan_bytes)?;

    // Prepare tokenizer
    let bpe = bpe(&plan.encoding)?;

    // Prepare output file (truncate if exists)
    let mut out = fs::File::create(out_path)?;

    // For each scenario, execute and measure
    for sc in plan
        .scenarios
        .iter()
    {
        let root = PathBuf::from(&sc.fixture_path);

        // Ensure symbol index exists - fail fast if this fails
        run_cmd_in(&root, "rup", &["symbols"])?;

        // Build common argv: ["context", <queries...>, ...]
        let mut base: Vec<String> = Vec::new();
        base.push("context".to_string());
        for q in &sc.queries
        {
            base.push(q.clone());
        }

        // Collect run outputs
        let mut run_jsons: Vec<Vec<u8>> = Vec::new();
        for r in sc
            .runs
            .iter()
        {
            let mut argv = base.clone();
            for a in &r.args
            {
                argv.push(a.clone());
            }

            // Add scenario tier or budget flags only if not already present in run args
            if !argv
                .iter()
                .any(|a| a == "--budget" || a == "--tier")
            {
                if let Some(b) = sc.budget
                {
                    argv.push("--budget".into());
                    argv.push(b.to_string());
                }
                else if let Some(t) = &sc.tier
                {
                    argv.push("--tier".into());
                    argv.push(t.clone());
                }
            }

            // Ensure JSON output and quiet/no-color flags; add if missing
            if !argv
                .iter()
                .any(|a| a == "--json")
            {
                argv.push("--json".into());
            }
            if !argv
                .iter()
                .any(|a| a == "--quiet")
            {
                argv.push("--quiet".into());
            }
            if !argv
                .iter()
                .any(|a| a == "--no-color")
            {
                argv.push("--no-color".into());
            }
            // Execute `rup context`
            let bytes = run_cmd_in(
                &root,
                "rup",
                &argv
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>(),
            )?;
            run_jsons.push(bytes);
        }

        // Determinism: if two runs, compare raw bytes
        let deterministic = if run_jsons.len() >= 2
        {
            run_jsons[0] == run_jsons[1]
        }
        else
        {
            true
        };

        // Use the first run as the measured output
        let first = parse_json_output(&run_jsons[0])?;

        // Baseline from full files referenced by items
        let paths = extract_paths(&first.items);
        let union = read_whole_files(&root, &paths)?;
        let baseline_tokens = count_tokens(&bpe, &union);

        // Actual tokens from tool's own tally
        let actual_tokens = first.total_tokens;

        // Metrics
        let row = ScoreRow {
            name: sc
                .name
                .clone(),
            encoding: plan
                .encoding
                .clone(),
            cef: cef(baseline_tokens, actual_tokens),
            dcr: dcr_from_items(&first.items),
            pfr: if sc.probe_first { 1.0 } else { 0.0 },
            deterministic_json_equal: deterministic,
            actual_tokens,
            baseline_tokens,
            within_budget: actual_tokens <= first.budget,
            items_count: first
                .items
                .len(),
            tier: sc
                .tier
                .clone(),
            budget: sc
                .budget
                .or(Some(first.budget)),
        };

        // Emit one compact JSON line per scenario
        let line = serde_json::to_string(&row)?;
        writeln!(out, "{line}")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn test_extract_paths_windows_parsing()
    {
        // Test Windows path parsing with drive letters
        let items = vec![
            JsonItem {
                id: "C:\\Users\\test\\file.rs:10-20".to_string(),
                tokens: 100,
            },
            JsonItem {
                id: "D:\\Projects\\src\\main.rs:5-15".to_string(),
                tokens: 150,
            },
            JsonItem {
                id: "/unix/path/file.py:1-10".to_string(),
                tokens: 80,
            },
        ];

        let paths = extract_paths(&items);

        // Should correctly split at LAST colon, preserving drive letters
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&"C:\\Users\\test\\file.rs".to_string()));
        assert!(paths.contains(&"D:\\Projects\\src\\main.rs".to_string()));
        assert!(paths.contains(&"/unix/path/file.py".to_string()));
    }

    #[test]
    fn test_extract_paths_edge_cases()
    {
        // Test edge cases for path parsing
        let items = vec![
            JsonItem { id: "file:10-20".to_string(), tokens: 50 }, // Simple case
            JsonItem {
                id: "path:with:colons:30-40".to_string(),
                tokens: 60,
            }, // Multiple colons
            JsonItem { id: "no_colon_at_all".to_string(), tokens: 70 }, // No colon (invalid)
            JsonItem { id: "".to_string(), tokens: 0 },            // Empty string
        ];

        let paths = extract_paths(&items);

        // Should handle cases correctly - splits at LAST colon only
        assert_eq!(paths.len(), 2); // Only valid entries with colons
        assert!(paths.contains(&"file".to_string()));
        assert!(paths.contains(&"path:with:colons".to_string())); // Preserves earlier colons
    }

    #[test]
    fn test_extract_paths_non_adjacent_deduplication()
    {
        // Test that HashSet properly deduplicates non-adjacent duplicates
        let items = vec![
            JsonItem { id: "src/lib.rs:1-10".to_string(), tokens: 100 },
            JsonItem { id: "src/main.rs:5-15".to_string(), tokens: 150 },
            JsonItem { id: "src/lib.rs:20-30".to_string(), tokens: 80 }, // Duplicate path
            JsonItem { id: "tests/mod.rs:1-5".to_string(), tokens: 50 },
            JsonItem { id: "src/main.rs:40-50".to_string(), tokens: 120 }, // Another duplicate
            JsonItem { id: "src/lib.rs:50-60".to_string(), tokens: 90 },   // Third occurrence
        ];

        let paths = extract_paths(&items);

        // Should deduplicate properly using HashSet - only unique paths
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&"src/lib.rs".to_string()));
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&"tests/mod.rs".to_string()));

        // Verify each path appears exactly once despite multiple item entries
        let lib_count = paths
            .iter()
            .filter(|p| *p == "src/lib.rs")
            .count();
        let main_count = paths
            .iter()
            .filter(|p| *p == "src/main.rs")
            .count();
        let test_count = paths
            .iter()
            .filter(|p| *p == "tests/mod.rs")
            .count();

        assert_eq!(lib_count, 1);
        assert_eq!(main_count, 1);
        assert_eq!(test_count, 1);
    }

    #[test]
    fn test_dcr_calculation()
    {
        // Test DCR (Duplicate Collapse Rate) calculation logic
        let items_no_dupes = vec![
            JsonItem { id: "file1:1-10".to_string(), tokens: 100 },
            JsonItem { id: "file2:1-10".to_string(), tokens: 150 },
            JsonItem { id: "file3:1-10".to_string(), tokens: 80 },
        ];
        assert_eq!(dcr_from_items(&items_no_dupes), 0.0); // No duplication

        let items_with_dupes = vec![
            JsonItem { id: "file1:1-10".to_string(), tokens: 100 },
            JsonItem { id: "file1:1-10".to_string(), tokens: 100 }, // Duplicate
            JsonItem { id: "file2:1-10".to_string(), tokens: 150 },
            JsonItem { id: "file2:1-10".to_string(), tokens: 150 }, // Duplicate
        ];
        // DCR = 1 - (2 unique / 4 total) = 1 - 0.5 = 0.5
        assert_eq!(dcr_from_items(&items_with_dupes), 0.5);

        // Edge case: empty items
        assert_eq!(dcr_from_items(&[]), 0.0);
    }

    #[test]
    fn test_cef_calculation()
    {
        // Test CEF (Context Efficiency Factor) calculation
        assert_eq!(cef(1000, 500), 2.0); // baseline > actual = high efficiency
        assert_eq!(cef(500, 1000), 0.5); // baseline < actual = low efficiency
        assert_eq!(cef(1000, 1000), 1.0); // equal = perfect efficiency
        assert_eq!(cef(1000, 0), 0.0); // Division by zero protection
        assert_eq!(cef(0, 100), 0.0); // Zero baseline
    }

    #[test]
    fn test_symbols_failure_is_fatal()
    {
        use std::path::Path;

        // Test that symbols command failure is properly detected as fatal
        let non_existent_path = Path::new("/this/path/does/not/exist/definitely/not");

        // This should return an error (fail-fast behavior)
        let result = run_cmd_in(non_existent_path, "rup", &["symbols"]);

        // Verify the error contains expected failure message
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("rup failed") || err_msg.contains("No such file"));
    }

    #[test]
    fn test_run_cmd_handles_failure()
    {
        use std::path::Path;

        // Test that run_cmd_in properly surfaces command failures
        let current_dir = Path::new(".");

        // Run a command that should fail (invalid rup subcommand)
        let result = run_cmd_in(current_dir, "rup", &["this_subcommand_does_not_exist"]);

        // Should return an error, not panic
        assert!(result.is_err());

        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("rup failed"));
    }

    #[test]
    fn test_quiet_no_color_auto_append()
    {
        // Test that --quiet and --no-color are automatically appended
        let mut argv = vec!["context".to_string(), "test_query".to_string(), "--json".to_string()];

        // Simulate the logic from main() that auto-appends flags
        if !argv
            .iter()
            .any(|a| a == "--quiet")
        {
            argv.push("--quiet".into());
        }
        if !argv
            .iter()
            .any(|a| a == "--no-color")
        {
            argv.push("--no-color".into());
        }

        assert!(argv.contains(&"--quiet".to_string()));
        assert!(argv.contains(&"--no-color".to_string()));
        assert!(argv.contains(&"--json".to_string()));

        // Test that existing flags are not duplicated
        let mut argv_with_existing = vec![
            "context".to_string(),
            "test_query".to_string(),
            "--json".to_string(),
            "--quiet".to_string(),
        ];

        if !argv_with_existing
            .iter()
            .any(|a| a == "--quiet")
        {
            argv_with_existing.push("--quiet".into());
        }
        if !argv_with_existing
            .iter()
            .any(|a| a == "--no-color")
        {
            argv_with_existing.push("--no-color".into());
        }

        // Should have --quiet once and --no-color once
        assert_eq!(
            argv_with_existing
                .iter()
                .filter(|a| *a == "--quiet")
                .count(),
            1
        );
        assert_eq!(
            argv_with_existing
                .iter()
                .filter(|a| *a == "--no-color")
                .count(),
            1
        );
    }
}
