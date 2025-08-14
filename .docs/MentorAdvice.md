Below is a focused, production-grade review of your Phase B2
backup-session management module. I group it into: (1) critical
correctness and safety fixes, (2) performance/scale
improvements, (3) API/UX refinements, and (4) ready-to-apply
patch snippets (with comments placed **above** the lines they
describe).

---

## 1) Critical correctness and safety fixes

1. Filter “latest” and “last-successful” to **completed sessions**
   Your resolver currently computes `latest`/`last-successful`
   from all indexed sessions, including incomplete ones. Prefer
   sessions that have a `DONE` marker; otherwise `show/restore`
   can fail on the resolved id.

2. Case-insensitive engine filtering
   `list_sessions_filtered` matches engine names exactly. Users
   often type `--engine=Auto`; make matching
   `eq_ignore_ascii_case` to avoid surprises.

3. Negative durations in `parse_relative_time`
   The parser accepts negative numbers (e.g., `-7d`) implicitly.
   Reject negatives explicitly; they invert the meaning of
   “since”.

4. Size computation excludes metadata files
   `calculate_session_size` currently includes `manifest.json`
   and `DONE`. Excluding them yields a clearer “payload size”
   signal.

---

## 2) Performance and scale improvements

5. Avoid manifest reads for **every** session on list
   `list_sessions_filtered` calls `read_session_manifest` per
   session just to derive `sample_paths`. That’s unnecessary and
   scales poorly. Filter and sort using **index entries only**,
   **truncate to limit**, then read manifests **only for the top
   N** to get sample paths. This keeps `list` sub-200 ms even with
   large indices.

6. Sort by parsed time (for robustness)
   Your timestamps sort correctly lexicographically given your
   current RFC 3339 format, but sorting by a parsed
   `DateTime<Utc>` prevents future regressions if the display
   format changes.

---

## 3) API and UX refinements

7. Aliases should respect completion policy
   Resolution for `latest`/`last-successful` should skip
   incomplete sessions without needing a separate flag; later you
   can add `--include-incomplete` behavior explicitly.

8. Ambiguity messages
   When fuzzy queries return multiple matches, present them
   sorted by timestamp (newest first) to aid quick disambiguation.

---

## 4) Ready-to-apply patch snippets

> Notes
> • Comments are placed **above** the code lines, per your style.
> • These edits are minimal and local.

### 4.1 Session resolution: prefer complete sessions, robust sort

```rust
// Ensure we can parse times once and reuse
use chrono::{DateTime, Utc};

// Resolve session ID (internal): prefer completed sessions when using aliases
fn resolve_session_id_internal(repo_root: &Path, query: &str) -> Result<SessionIdResolution> {
    // Read index entries once
    let sessions = list_sessions(repo_root)?;

    // Helper to check completion
    // (Avoid re-reading manifests; DONE marker is enough)
    let is_complete = |id: &str| session_is_complete(repo_root, id).unwrap_or(false);

    // Precompute parsed timestamps (skip invalid safely)
    // and carry completion status to avoid repeated IO.
    let mut entries: Vec<(String, String, bool, Option<DateTime<Utc>>)> = sessions.iter().map(|s| {
        // Parse RFC3339; if parsing fails, None so it sorts last
        let parsed = DateTime::parse_from_rfc3339(&s.timestamp)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
        (s.id.clone(), s.timestamp.clone(), s.success, parsed)
    }).collect();

    // Handle aliases first
    match query {
        // latest: choose newest completed session by parsed time
        "latest" => {
            // Filter completed
            let mut cands: Vec<_> = entries.iter()
                .filter(|(id, _, _, _)| is_complete(id))
                .collect();
            // Sort by parsed time desc, then by string desc as tiebreaker
            cands.sort_by(|a, b| {
                b.3.cmp(&a.3).then_with(|| b.1.cmp(&a.1))
            });
            return Ok(match cands.first() {
                Some((id, ..)) => SessionIdResolution::Single(id.clone()),
                None => SessionIdResolution::NotFound,
            });
        }
        // last-successful: newest completed AND success=true
        "last-successful" => {
            let mut cands: Vec<_> = entries.iter()
                .filter(|(id, _, success, _)| *success && is_complete(id))
                .collect();
            cands.sort_by(|a, b| {
                b.3.cmp(&a.3).then_with(|| b.1.cmp(&a.1))
            });
            return Ok(match cands.first() {
                Some((id, ..)) => SessionIdResolution::Single(id.clone()),
                None => SessionIdResolution::NotFound,
            });
        }
        _ => {}
    }

    // Collect matches (exact, short-suffix, date-prefix)
    let mut matches: Vec<(String, Option<DateTime<Utc>>, String)> = Vec::new();

    for (id, ts, _success, parsed) in &entries {
        // Exact match
        if id == query {
            return Ok(SessionIdResolution::Single(id.clone()));
        }
        // Short ID (require a minimal length to reduce noise)
        if query.len() >= 8 && id.ends_with(query) {
            matches.push((id.clone(), *parsed, ts.clone()));
        }
        // Date prefix like "2025-08-14"
        if query.contains('-') && id.starts_with(query) {
            matches.push((id.clone(), *parsed, ts.clone()));
        }
    }

    // Sort matches newest-first for better ambiguity messages
    matches.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.cmp(&a.2)));

    // Return resolution
    Ok(match matches.len() {
        0 => SessionIdResolution::NotFound,
        1 => SessionIdResolution::Single(matches[0].0.clone()),
        _ => SessionIdResolution::Multiple(matches.into_iter().map(|(id, _, _)| id).collect()),
    })
}
```

### 4.2 Listing: filter/sort first; read manifests only for top-N sample paths; case-insensitive engine filter

```rust
// List sessions with filters, minimizing manifest IO
pub fn list_sessions_filtered(repo_root: &Path, req: ListRequest) -> Result<Vec<SessionInfo>> {
    // Parse "since" once
    let since_time = if let Some(ref s) = req.since {
        Some(parse_relative_time(s)?)
    } else {
        None
    };

    // Load index entries
    let mut entries = list_sessions(repo_root)?;

    // Keep only completed sessions
    entries.retain(|e| session_is_complete(repo_root, &e.id).unwrap_or(false));

    // Apply filters that require only index data
    if req.successful {
        entries.retain(|e| e.success);
    }

    if let Some(ref engine_filter) = req.engine {
        // Case-insensitive engine matching
        let target = engine_filter.to_ascii_lowercase();
        entries.retain(|e| e.engine.to_ascii_lowercase() == target);
    }

    if let Some(since) = since_time {
        // Drop sessions older than the bound
        entries.retain(|e| {
            DateTime::parse_from_rfc3339(&e.timestamp)
                .ok()
                .map(|dt| dt.with_timezone(&Utc) >= since)
                .unwrap_or(false)
        });
    }

    // Sort by parsed timestamp desc/asc robustly
    entries.sort_by(|a, b| {
        let ap = DateTime::parse_from_rfc3339(&a.timestamp).ok().map(|x| x.with_timezone(&Utc));
        let bp = DateTime::parse_from_rfc3339(&b.timestamp).ok().map(|x| x.with_timezone(&Utc));
        if req.sort_desc {
            bp.cmp(&ap).then_with(|| b.timestamp.cmp(&a.timestamp))
        } else {
            ap.cmp(&bp).then_with(|| a.timestamp.cmp(&b.timestamp))
        }
    });

    // Truncate to limit before manifest reads
    if entries.len() > req.limit {
        entries.truncate(req.limit);
    }

    // Collect SessionInfo; now read manifests only for sample paths
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        // Try to read manifest to extract first 3 sample paths
        let sample_paths = match read_session_manifest(repo_root, &e.id) {
            Ok(m) => m.files.iter()
                .take(3)
                .map(|f| f.rel_path.display().to_string())
                .collect(),
            Err(_) => Vec::new(),
        };

        out.push(SessionInfo {
            id: e.id,
            timestamp: e.timestamp,
            engine: e.engine,
            success: e.success,
            files: e.files,
            sample_paths,
        });
    }

    Ok(out)
}
```

### 4.3 Relative time parsing: reject negatives

```rust
// Parse relative time specifications like "7d", "24h"
fn parse_relative_time(time_str: &str) -> Result<DateTime<Utc>> {
    // Trim and validate
    let time_str = time_str.trim();
    if time_str.is_empty() {
        bail!("Empty time specification");
    }

    // Split number and unit
    let (number_str, unit) = match time_str.chars().last() {
        Some('d' | 'h' | 'm' | 's') => (&time_str[..time_str.len() - 1], time_str.chars().last().unwrap()),
        _ => bail!("Invalid time unit in '{}'. Use d, h, m, or s", time_str),
    };

    // Parse and reject negatives
    let number: i64 = number_str.parse()
        .with_context(|| format!("Invalid number '{}' in time specification", number_str))?;
    if number < 0 {
        bail!("Negative durations are not allowed: '{}'", time_str);
    }

    // Map to chrono::Duration
    let duration = match unit {
        'd' => Duration::days(number),
        'h' => Duration::hours(number),
        'm' => Duration::minutes(number),
        's' => Duration::seconds(number),
        _ => unreachable!(),
    };

    // Compute bound
    Ok(Utc::now() - duration)
}
```

### 4.4 Session size: exclude `manifest.json` and `DONE`

```rust
// Compute size of backed-up payload (exclude manifest and DONE)
fn calculate_session_size(session_path: &Path) -> Result<u64> {
    // Accumulator
    let mut total_size = 0u64;

    // Recursive visitor
    fn visit_dir(dir: &Path, total: &mut u64) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let md = entry.metadata()?;

            // Recurse into directories
            if md.is_dir() {
                visit_dir(&path, total)?;
                continue;
            }

            // Skip metadata files
            let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if fname == "manifest.json" || fname == "DONE" {
                continue;
            }

            // Sum file size
            *total += md.len();
        }
        Ok(())
    }

    // If session dir exists, walk it
    if session_path.exists() {
        visit_dir(session_path, &mut total_size)?;
    }

    Ok(total_size)
}
```

### 4.5 Minor hygiene: remove unused imports

```rust
// Remove this line if not used
// use std::collections::HashMap;
```

---

## Optional niceties (nice-to-have, not blocking)

- When ambiguity occurs in `resolve_session_id`, include the
  **engine** and **success** flag in the error message for faster
  eyeballing.
- Add a `--limit` default to `50` rather than `100` to keep output
  dense by default; users who need more can raise it.

---

## Test additions (high signal)

- **latest/last-successful**: create (1) incomplete recent session,
  (2) completed earlier session; assert that `latest` chooses the
  completed one.
- **engine filter case**: create sessions with `Auto`, `auto`,
  `AUTO`; assert filters match regardless of case.
- **list manifest IO**: add a metric counter or debug hook to
  ensure manifests are only read for the **top N** after
  filtering.
- **negative durations**: assert `-7d` fails.
- **size calculation**: assert `manifest.json` and `DONE` are
  excluded.

---

## Summary

Your module is already well-structured and matches the planned
Phase B2 UX. The fixes above tighten correctness (completion
policy, negative durations), improve performance at scale
(limit-before-manifest-read), and refine user expectations
(case-insensitive engine filter, payload size semantics). The
patches are deliberately minimal and local, so you can apply them
without rippling through your CLI layer.
