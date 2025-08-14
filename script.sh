#!/usr/bin/env bash
# -----------------------------------------------------------------------------
# Generate context for Phase 3 implementation - Smart Context Assembly
# Extracts core architecture, existing patterns, and Phase 3 implementation points
# -----------------------------------------------------------------------------

set -Eeuo pipefail

# ---- config ---------------------------------------------------------------
OUT_PATH="${OUT_PATH:-context/phase3_context.txt}"
CONTEXT_DIR="$(dirname "$OUT_PATH")"

if [[ -n "${CORE_FILES-}" ]]; then
  FILES=($CORE_FILES)  # shellcheck disable=SC2206
else
# Phase 3 relevant files - existing architecture + implementation points
  FILES=(
    "Cargo.toml"
    "src/lib.rs" 
    "src/main.rs"
    "src/cli.rs"
    "src/core/symbols.rs"
    "src/core/extract/mod.rs"
    "src/core/extract/target.rs"
    "src/core/chunk.rs"
    "src/infra/config.rs"
    "src/infra/utils.rs"
    "src/infra/walk.rs"
    "src/parsers/rust_parser.rs"
    "src/parsers/python_parser.rs"
    "TODO.md"
    "CLAUDE.md"
  )
fi

# ---- helpers --------------------------------------------------------------
die() { echo "[gen_core] error: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing '$1'"; }
nlines() { wc -l < "$1" | tr -d '[:space:]'; }

# ---- preflight ------------------------------------------------------------
need cargo
mkdir -p "$CONTEXT_DIR"

TARGETS=()
declare -A TOTALS
for f in "${FILES[@]}"; do
  [[ -f "$f" ]] || die "file not found: $f"
  lc="$(nlines "$f")"
  [[ "$lc" =~ ^[0-9]+$ ]] || die "bad line count: $f"
  TARGETS+=("${f}:1-${lc}")
  TOTALS["$f"]="$lc"
done

# ---- header ---------------------------------------------------------------
TMP_HEADER="$(mktemp)"
{
  echo "=== Phase 3: Smart Context Assembly - Implementation Context ==="
  echo "generated: $(date -u +'%Y-%m-%dT%H:%M:%SZ')"
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "commit: $(git rev-parse --short HEAD)"
    echo "toplevel: $(git rev-parse --show-toplevel)"
  fi
  echo ""
  echo "OBJECTIVE: Implement Phase 3 - Smart Context Assembly"
  echo "- symbol_index.rs: load symbols.jsonl, exact/fuzzy lookup, spans"
  echo "- budgeter.rs: tiktoken-rs estimation, deterministic ordering"  
  echo "- CLI integration: rup context --budget --template --semantic"
  echo "- Performance: <2s typical, <5s heavy, ±10% accuracy"
  echo ""
  echo "files:"
  for f in "${FILES[@]}"; do
    echo "  - ${f}"
  done
  echo "============================================================="
  echo
} > "$TMP_HEADER"

# ---- extract --------------------------------------------------------------
TMP_EXTRACT="$(mktemp)"
cargo run --quiet -- extract "${TARGETS[@]}" \
  --annotate --fence --output "$TMP_EXTRACT"

# ---- numbering ------------------------------------------------------------
TMP_NUMBERED="$(mktemp)"
awk -v totals_map="$(for k in "${!TOTALS[@]}"; do echo "$k:${TOTALS[$k]}"; done)" '
BEGIN {
  in_code=0; base=1; pending_base=0; cur_path=""; total_lines=0
  split(totals_map, lines, "\n")
  for (i in lines) {
    split(lines[i], kv, ":")
    if (kv[1] != "") totals[kv[1]] = kv[2]
  }
}
/[^`]*[A-Za-z0-9_.\/-]+:[0-9]+-[0-9]+/ {
  match($0, /([A-Za-z0-9_.\/-]+):([0-9]+)-([0-9]+)/, m)
  if (m[1] != "") cur_path = m[1]
  if (m[2] != "") pending_base = m[2] + 0
  total_lines = (cur_path in totals) ? totals[cur_path] : 0
}
$0 ~ /^```/ {
  if (!in_code) {
    in_code=1; base=(pending_base>0 ? pending_base : 1); pending_base=0
    print $0
    if (cur_path != "")
      printf("// SOURCE %s  L%d… / %d\n", cur_path, base, total_lines)
    next
  } else { in_code=0; print $0; next }
}
in_code==1 { printf("%6d | %s\n", base, $0); base++; next }
{ print }
' "$TMP_EXTRACT" > "$TMP_NUMBERED"

# ---- stitch ---------------------------------------------------------------
TMP_OUT="$(mktemp)"
cat "$TMP_HEADER" "$TMP_NUMBERED" > "$TMP_OUT"
mv -f "$TMP_OUT" "$OUT_PATH"

rm -f "$TMP_HEADER" "$TMP_EXTRACT" "$TMP_NUMBERED"
echo "[gen_core] wrote: $OUT_PATH"
