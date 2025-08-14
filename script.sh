# 0) Where outputs go
mkdir -p context

# 1) Project overview (tree)
cargo run -- tree --depth 3 --ignore target --ignore .git > context/tree.txt

# 2) Symbol inventory (Rust + Python)
cargo run -- symbols . --languages rust python --output context/symbols.jsonl

# 3) Core modules into one annotated, fenced file
#    We extract full files by computing line counts first.
core_files=(
  src/cli.rs
  src/core/edit.rs
  src/core/patch.rs
  src/core/git.rs
  src/core/apply_engine.rs
)
: > context/core.txt
for f in "${core_files[@]}"; do
  [ -f "$f" ] || continue
  n=$(wc -l < "$f")
  cargo run -- extract "$f:1-$n" --annotate --fence --output context/core.txt
done

# 4) Chunk the core bundle for LLM review (o200k_base = tokenizer-encoding mode)
cargo run -- chunk context/core.txt \
  --model o200k_base \
  --max-tokens 5000 \
  --overlap 200 \
  --by-symbols true \
  --output-dir context/chunks

# 5) (Optional) Preview a spec as a real unified diff using the new engines.
#    Put your EBNF edit spec in edits/sample.rup (or change the path).
if [ -f edits/sample.rup ]; then
  cargo run -- preview \
    edits/sample.rup \
    --engine auto \
    --git-mode 3way \
    --whitespace nowarn \
    --show-diff true \
    --repo-root . > context/preview.diff
fi
