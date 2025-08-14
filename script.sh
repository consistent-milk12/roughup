#!/usr/bin/env bash
# -----------------------------------------------------------------------------
# Generate "context/core.txt" by extracting full, annotated sources for the
# core Phase-2 files in a single roughup invocation.
#
# Usage:
#   scripts/gen_core.sh
#
# Environment overrides:
#   OUT_PATH   : output file (default: context/core.txt)
#   CORE_FILES : space-separated list of files to include, in order
#                default:
#                  src/cli.rs
#                  src/core/edit.rs
#                  src/core/patch.rs
#                  src/core/git.rs
#                  src/core/apply_engine.rs
# -----------------------------------------------------------------------------

set -Eeuo pipefail

# ---- config ---------------------------------------------------------------

OUT_PATH="${OUT_PATH:-context/core.txt}"        # output bundle path
CONTEXT_DIR="$(dirname "$OUT_PATH")"            # output directory

# Default file list; override via CORE_FILES env if needed
if [[ -n "${CORE_FILES-}" ]]; then
  # shellcheck disable=SC2206
  FILES=($CORE_FILES)                            # honor caller order
else
  FILES=(
    "src/cli.rs"
    "src/core/edit.rs"
    "src/core/patch.rs"
    "src/core/git.rs"
    "src/core/apply_engine.rs"
  )
fi

# ---- helpers --------------------------------------------------------------

die() {
  echo "[gen_core] error: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 \
    || die "missing '$1' in PATH"
}

nlines() {
  # Print the line count of a file (trim spaces)
  wc -l < "$1" | tr -d '[:space:]'
}

# ---- preflight ------------------------------------------------------------

need cargo

mkdir -p "$CONTEXT_DIR"

# Validate files and build extract targets
TARGETS=()
for f in "${FILES[@]}"; do
  [[ -f "$f" ]] || die "file not found: $f"
  lc="$(nlines "$f")"
  [[ "$lc" =~ ^[0-9]+$ ]] || die "bad line count: $f"
  TARGETS+=("${f}:1-${lc}")
done

# ---- header ---------------------------------------------------------------

# Write a small provenance header; we will prepend it later
TMP_HEADER="$(mktemp)"
{
  echo "=== core bundle (roughup extract) ==="
  echo "generated: $(date -u +'%Y-%m-%dT%H:%M:%SZ')"
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "commit: $(git rev-parse --short HEAD)"
    echo "toplevel: $(git rev-parse --show-toplevel)"
  fi
  echo "files:"
  for f in "${FILES[@]}"; do
    echo "  - ${f}"
  done
  echo "====================================="
  echo
} > "$TMP_HEADER"

# ---- extract --------------------------------------------------------------

# Use a temp file for roughup output, then prepend the header atomically
TMP_EXTRACT="$(mktemp)"

# One invocation; ordered targets; annotated + fenced
cargo run --quiet -- \
  extract "${TARGETS[@]}" \
  --annotate \
  --fence \
  --output "$TMP_EXTRACT"

# Stitch header + extract into final OUT_PATH atomically
TMP_OUT="$(mktemp)"
cat "$TMP_HEADER" "$TMP_EXTRACT" > "$TMP_OUT"
mv -f "$TMP_OUT" "$OUT_PATH"

# Cleanup temp files
rm -f "$TMP_HEADER" "$TMP_EXTRACT"

echo "[gen_core] wrote: $OUT_PATH"
