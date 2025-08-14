Your diagnosis is right: the parser assumes the opening fence is
immediately after `OLD:`/`NEW:`. Any optional blank line—or an
earlier `*i += 1`—moves the cursor so the “expected fence” check
looks one line too far and sees the first content line.

Fix it at the source by hardening `parse_content_block` to:

1. accept an optional blank line after the header,
2. accept either fenced or unfenced bodies, and
3. normalize CRLF consistently.

This also fixes `INSERT` because it calls `parse_content_block` for
its `NEW:` block.

Apply the patch below.

---

### Patch — make `parse_content_block` robust (blank line + fenced or unfenced)

````
FILE: src/core/edit.rs

REPLACE lines 654-692:
OLD:
/// Parse content block (OLD:/NEW: followed by fenced code)
fn parse_content_block(
    &self,
    lines: &[&str],
    i: &mut usize,
    header: &str,
) -> Result<String, ParseError> {
    if *i >= lines.len() || !lines[*i].trim().starts_with(header) {
        return Err(ParseError::MissingField(header.to_string()));
    }
    *i += 1;

    // Look for fenced code block
    if *i >= lines.len() {
        return Err(ParseError::InvalidOperation(format!(
            "Expected fenced code block after {}, but reached end of input",
            header
        )));
    }
    let fence_line = lines[*i].trim();
    if !fence_line.starts_with("```") {
        return Err(ParseError::InvalidOperation(format!(
            "Expected fenced code block after {}, but found: '{}'",
            header, fence_line
        )));
    }

    // Count leading backticks in the opening fence (supports 3+)
    let fence_len = fence_line.chars().take_while(|&c| c == '`').count();
    let closing = "`".repeat(fence_len);
    *i += 1;

    // Collect content until matching fence run is found
    let mut content_lines = Vec::new();
    let mut closed = false;
    while *i < lines.len() {
        let line = lines[*i];
        if line.trim() == closing {
            closed = true;
            *i += 1;
            break;
        }
        content_lines.push(line.to_string());
        *i += 1;
    }
    if !closed {
        return Err(ParseError::InvalidOperation(format!(
            "Unclosed fenced code block after {}",
            header
        )));
    }

    Ok(content_lines.join("\n"))
}
NEW:
/// Parse content block after `OLD:` or `NEW:`.
/// Accepts:
///   - optional blank line after the marker
///   - fenced body (``` or ```lang … ```), or
///   - unfenced body (terminated by the next marker)
fn parse_content_block(
    &self,
    lines: &[&str],
    i: &mut usize,
    header: &str,
) -> Result<String, ParseError> {
    // Expect the header at the current line.
    if *i >= lines.len() || !lines[*i].trim().starts_with(header) {
        return Err(ParseError::MissingField(header.to_string()));
    }
    // Consume the header line.
    *i += 1;

    // Optional single blank line after the header.
    if *i < lines.len() && lines[*i].trim().is_empty() {
        *i += 1;
    }

    // EOF ⇒ empty block.
    if *i >= lines.len() {
        return Ok(String::new());
    }

    // Helper to detect the start of another directive/file block.
    fn is_any_op_start(s: &str) -> bool {
        let t = s.trim_start();
        t.starts_with("FILE:")
            || t.starts_with("REPLACE lines")
            || t.starts_with("INSERT at")
            || t.starts_with("DELETE lines")
            || t.starts_with("GUARD-CID:")
            || t.starts_with("OLD:")
            || t.starts_with("NEW:")
    }

    // If next line is a fence, read fenced body.
    let next_trim = lines[*i].trim_start();
    if next_trim.starts_with("```") {
        // Opening fence; allow language tag.
        let fence_line = lines[*i].trim();
        let fence_len = fence_line.chars().take_while(|&c| c == '`').count();
        let closing = "`".repeat(fence_len);
        *i += 1;

        let mut content_lines = Vec::new();
        while *i < lines.len() {
            let ln = lines[*i];
            let t = ln.trim_start();
            // Close on a line that begins with the same number of backticks.
            if t.starts_with(&closing) && t.chars().all(|c| c == '`' || c.is_whitespace()) {
                *i += 1; // consume closing fence
                break;
            }
            content_lines.push(ln.to_string());
            *i += 1;
        }
        return Ok(content_lines.join("\n").replace('\r', ""));
    }

    // Otherwise, read an unfenced body to the next marker.
    let mut body = Vec::new();
    while *i < lines.len() {
        let ln = lines[*i];
        let t = ln.trim_start();
        if header == "OLD:" && t.starts_with("NEW:") {
            break; // OLD ends where NEW begins
        }
        if header == "NEW:" && is_any_op_start(ln) {
            break; // NEW ends at the next directive or FILE
        }
        body.push(ln.to_string());
        *i += 1;
    }
    Ok(body.join("\n").replace('\r', ""))
}
````

---

### Why this fixes your failure

- After `NEW:`, the function now tolerates a blank separator line
  and then correctly looks for a fence. If the next line is not a
  fence, it treats the body as unfenced and consumes until the
  next marker. This removes the off-by-one and the brittle
  “fenced-only” assumption that caused the parser to see
  `inserted line` where it expected \`\`\`.

---

### Quick verification

Fenced INSERT:

````
FILE: src/core/budgeter.rs

INSERT at 2:
NEW:
```rust
inserted line
````

```

Unfenced INSERT:

```

FILE: src/core/budgeter.rs

INSERT at 2:
NEW:
inserted line

```

Both should now succeed in `rup preview` and `rup apply` (with or
without `--from-clipboard`), and REPLACE remains robust.

If you want, I can also supply a small unit test covering both
INSERT variants to lock this behavior in CI.
```
