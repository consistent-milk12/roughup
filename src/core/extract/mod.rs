//! Line-range extractor with token budgeting and compaction.
//!
//! Features:
//! - context expansion (--context)
//! - merge nearby ranges (--merge-within)
//! - whitespace compaction (--dedent, --squeeze-blank)
//! - token budgeting (--budget, --model) using core::budgeter
//! - hard/priority ranges via "!" prefix in the targets spec
//! - honors --annotate, --fence, --clipboard

pub mod target;

pub use target::ExtractionTarget;

use anyhow::{Context, Result, anyhow};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{AppContext, ExtractArgs};
use crate::core::budgeter::{Budgeter, Item as BudgetItem, Priority};
use crate::infra::io::read_file_smart;

#[derive(Debug, Clone)]
struct Span {
    start: usize, // 1-based inclusive
    end: usize,   // 1-based inclusive
    hard: bool,   // true when user prefixed with '!'
}

#[derive(Debug, Clone)]
struct FileSpec {
    path: PathBuf,
    spans: Vec<Span>,
}

pub fn run(args: ExtractArgs, ctx: &AppContext) -> Result<()> {
    // Parse all target specs into file->spans
    let mut by_file: BTreeMap<PathBuf, Vec<Span>> = BTreeMap::new();
    for spec in &args.targets {
        let parsed =
            parse_target_spec(spec).with_context(|| format!("invalid target spec: '{spec}'"))?;
        by_file.entry(parsed.path).or_default().extend(parsed.spans);
    }

    // Expand context & merge per-file
    for spans in by_file.values_mut() {
        expand_context(spans, args.context);
        merge_spans(spans, args.merge_within);
    }

    // Build budget items (one per merged span)
    let mut items: Vec<BudgetItem> = Vec::new();

    for (path, spans) in &by_file {
        // Read file once
        let content =
            read_file_smart(path).with_context(|| format!("reading {}", path.display()))?;
        let text = content.as_ref();

        for s in spans {
            let raw = slice_lines(text, s.start, s.end);
            let mut body = raw;

            // Compaction
            if args.dedent {
                body = dedent(&body);
            }
            if args.squeeze_blank {
                body = squeeze_blank_lines(&body);
            }

            // Render snippet
            let snippet = render_snippet(path, s.start, s.end, &body, args.fence, args.annotate);

            let id = format!("{}:{}-{}", path.display(), s.start, s.end);
            // Heuristic: hard items get a small "must keep" floor
            let min_tokens = if s.hard { 64 } else { 0 };

            items.push(BudgetItem {
                id,
                content: snippet,
                priority: if s.hard {
                    Priority::High
                } else {
                    Priority::Medium
                },
                hard: s.hard,
                min_tokens,
            });
        }
    }

    // Token budgeting
    let final_text = if let Some(budget) = args.budget {
        let b = Budgeter::new(&args.model)
            .with_context(|| format!("loading tokenizer for '{}'", args.model))?;
        let fit = b.fit(items, budget)?;
        join_fitted(&fit.items)
    } else {
        // No budget: join in deterministic order (priority desc, id asc)
        items.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
        let contents: Vec<&str> = items.iter().map(|i| i.content.as_str()).collect();
        contents.join("\n")
    };

    // Write
    if !ctx.quiet {
        println!("Writing {}", args.output.display());
    }

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("mkdir -p {}", parent.display()))?;
    }

    fs::write(&args.output, &final_text)
        .with_context(|| format!("write {}", args.output.display()))?;

    // Optional clipboard
    if args.clipboard {
        copy_to_clipboard(&final_text)?;
        if !ctx.quiet {
            println!("✓ Copied to clipboard");
        }
    }

    if !ctx.quiet {
        println!("✓ Done");
    }
    Ok(())
}

fn parse_target_spec(s: &str) -> Result<FileSpec> {
    // Split on the first ':' (Windows drive letters contain ':', so handle gracefully)
    // Heuristic: if there are multiple ':', use the last as the range separator.
    let (path_part, ranges_part) = if let Some(idx) = s.rfind(':') {
        (&s[..idx], &s[idx + 1..])
    } else {
        return Err(anyhow!("missing ':' separating file and ranges"));
    };

    let path = PathBuf::from(path_part.trim());
    if path.as_os_str().is_empty() {
        return Err(anyhow!("empty path"));
    }
    if ranges_part.trim().is_empty() {
        return Err(anyhow!("missing ranges after ':'"));
    }

    let mut spans = Vec::new();
    for raw in ranges_part.split(',') {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }
        let (hard, t) = if let Some(stripped) = t.strip_prefix('!') {
            (true, stripped.trim())
        } else {
            (false, t)
        };

        // forms: "A-B", "A", "A+N"
        let (start, end) = if let Some(p) = t.find('-') {
            let a = t[..p].trim().parse::<usize>()?;
            let b = t[p + 1..].trim().parse::<usize>()?;
            (a, b)
        } else if let Some(p) = t.find('+') {
            let a = t[..p].trim().parse::<usize>()?;
            let n = t[p + 1..].trim().parse::<usize>()?;
            (a, a.saturating_add(n.saturating_sub(1)))
        } else {
            let a = t.parse::<usize>()?;
            (a, a)
        };

        if start == 0 || end == 0 {
            return Err(anyhow!("lines are 1-based; got 0 in '{t}'"));
        }
        spans.push(Span {
            start: start.min(end),
            end: start.max(end),
            hard,
        });
    }

    if spans.is_empty() {
        return Err(anyhow!("no spans found"));
    }

    Ok(FileSpec { path, spans })
}

fn expand_context(spans: &mut [Span], ctx: usize) {
    if ctx == 0 {
        return;
    }
    for s in spans.iter_mut() {
        s.start = s.start.saturating_sub(ctx);
        s.end = s.end.saturating_add(ctx);
        if s.start == 0 {
            s.start = 1;
        }
    }
}

fn merge_spans(spans: &mut Vec<Span>, within: usize) {
    if spans.is_empty() {
        return;
    }
    spans.sort_by_key(|s| (s.start, s.end));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    let mut cur = spans[0].clone();

    for s in spans.iter().skip(1) {
        if s.start <= cur.end.saturating_add(within + 1) {
            // overlap / near — merge
            cur.end = cur.end.max(s.end);
            cur.hard = cur.hard || s.hard;
        } else {
            out.push(cur);
            cur = s.clone();
        }
    }
    out.push(cur);
    *spans = out;
}

fn slice_lines(src: &str, start: usize, end: usize) -> String {
    // start/end are 1-based inclusive
    let sidx = start.saturating_sub(1);
    let eidx = end.saturating_sub(1);

    let mut out = String::new();
    for (i, line) in src.lines().enumerate() {
        if i < sidx {
            continue;
        }
        if i > eidx {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

fn dedent(s: &str) -> String {
    let mut min_indent: Option<usize> = None;
    for line in s.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let n = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        min_indent = Some(min_indent.map_or(n, |m| m.min(n)));
    }
    let k = min_indent.unwrap_or(0);
    if k == 0 {
        return s.to_string();
    }

    s.lines()
        .map(|l| {
            let mut cnt = 0usize;
            let mut out = String::new();
            for ch in l.chars() {
                if (ch == ' ' || ch == '\t') && cnt < k {
                    cnt += 1;
                    continue;
                }
                out.push(ch);
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn squeeze_blank_lines(s: &str) -> String {
    let mut out = String::new();
    let mut prev_blank = false;
    for line in s.lines() {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        prev_blank = blank;
    }
    out
}

fn render_snippet(
    path: &Path,
    start: usize,
    end: usize,
    body: &str,
    fence: bool,
    annotate: bool,
) -> String {
    let mut out = String::new();
    let lang = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if fence {
        out.push_str("```");
        out.push_str(lang);
        out.push('\n');
        if annotate {
            out.push_str(&annot_line(
                lang,
                &format!("{}:{}-{}", path.display(), start, end),
            ));
            out.push('\n');
        }
        out.push_str(body);
        out.push('\n');
        out.push_str("```");
    } else {
        if annotate {
            out.push_str(&format!(">>> {}:{}-{}\n", path.display(), start, end));
        }
        out.push_str(body);
    }
    out
}

fn annot_line(lang: &str, text: &str) -> String {
    match lang {
        "rs" | "js" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "h" | "hpp" | "go" | "java" | "kt" => {
            format!("// {text}")
        }
        "py" | "rb" | "sh" | "bash" | "zsh" | "yaml" | "yml" | "toml" | "ini" => {
            format!("# {text}")
        }
        _ => format!("// {text}"),
    }
}

fn join_fitted(items: &[crate::core::budgeter::FittedItem]) -> String {
    let parts: Vec<&str> = items.iter().map(|x| x.content.as_str()).collect();
    parts.join("\n")
}

// Minimal clipboard hook — if you already have one, feel free to replace.
fn copy_to_clipboard(s: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new().context("clipboard init")?;
    cb.set_text(s.to_string()).context("clipboard set")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hard_and_plus() {
        let f = parse_target_spec("src/lib.rs:!10-40,200+5,9").unwrap();
        assert_eq!(f.spans.len(), 3);
        assert!(f.spans[0].hard);
        assert_eq!((f.spans[1].start, f.spans[1].end), (200, 204));
        assert_eq!((f.spans[2].start, f.spans[2].end), (9, 9));
    }

    #[test]
    fn merging_and_context() {
        let mut v = vec![
            Span {
                start: 10,
                end: 20,
                hard: false,
            },
            Span {
                start: 23,
                end: 25,
                hard: false,
            },
        ];
        expand_context(&mut v, 2); // -> [8..22], [21..27]
        merge_spans(&mut v, 0); // overlap => merge into [8..27]
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].start, v[0].end), (8, 27));
    }

    #[test]
    fn squeeze_and_dedent() {
        let s = "    fn x() {}\n\n\n    let a = 1;\n";
        let d = dedent(s);
        assert!(d.starts_with("fn x()"));
        let q = squeeze_blank_lines(&d);
        assert_eq!(q.matches('\n').count(), 2); // two newlines total
    }
}
