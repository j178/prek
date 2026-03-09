#!/usr/bin/env bash
set -euo pipefail

TARGET_WORKSPACE=${HYPERFINE_BENCHMARK_WORKSPACE:?HYPERFINE_BENCHMARK_WORKSPACE is required}
COMMENT=${HYPERFINE_RESULTS_FILE:?HYPERFINE_RESULTS_FILE is required}
HEAD_BINARY=${HYPERFINE_HEAD_BINARY:?HYPERFINE_HEAD_BINARY is required}
BASE_BINARY=${HYPERFINE_BASE_BINARY:?HYPERFINE_BASE_BINARY is required}
REPO_WORKSPACE=$(pwd)
OUT_DIR=$(dirname "$COMMENT")
META_WORKSPACE="${TARGET_WORKSPACE}-meta"

failed=false

mkdir -p "$OUT_DIR"
OUT_MD="$OUT_DIR/out.md"
OUT_JSON="$OUT_DIR/out.json"

CURRENT_PREK_VERSION=$(
  "$HEAD_BINARY" --version | sed -n '1p'
)

write_line() {
  printf '%s\n' "$1" >> "$COMMENT"
}

write_blank_line() {
  printf '\n' >> "$COMMENT"
}

write_section() {
  local title="$1"
  local description="${2:-}"

  write_blank_line
  write_line "## $title"
  if [ -n "$description" ]; then
    write_line "$description"
  fi
}

# Compare the two commands in out.json (reference vs current).
# Hyperfine's JSON has results[0] = reference and results[1] = current.
# A ratio > 1 means current is slower (regression), < 1 means faster (improvement).
check_variance() {
  local cmd="$1"
  local num_results
  num_results=$(jq '.results | length' "$OUT_JSON")

  if [ "$num_results" -lt 2 ]; then
    return
  fi

  local ref_mean current_mean ratio pct
  ref_mean=$(jq '.results[0].mean' "$OUT_JSON")
  current_mean=$(jq '.results[1].mean' "$OUT_JSON")
  ratio=$(echo "scale=4; $current_mean / $ref_mean" | bc)
  pct=$(echo "scale=2; ($ratio - 1) * 100" | bc)

  if (( $(echo "${pct#-} > 10" | bc -l) )); then
    if (( $(echo "$ratio < 1" | bc -l) )); then
      write_line "✅  Performance improvement for \`$cmd\`: ${pct#-}% faster"
    else
      write_line "⚠️  Warning: Performance regression for \`$cmd\`: ${pct}% slower"
      failed=true
    fi
  fi
}

write_benchmark_details() {
  write_line "<details>"
  write_line "<summary>Benchmark details</summary>"
  write_blank_line
  cat "$OUT_MD" >> "$COMMENT"
  write_blank_line
  write_line "</details>"
}

benchmark() {
  local label="$1"
  local cmd="$2"
  local warmup="${3:-3}"
  local runs="${4:-30}"
  local setup="${5:-}"
  local prepare="${6:-}"
  local check_change="${7:-false}"
  local -a hyperfine_args=(-i -N -w "$warmup" -r "$runs" --export-markdown "$OUT_MD" --export-json "$OUT_JSON" --show-output)

  if [ -n "$setup" ]; then
    hyperfine_args+=(--setup "$setup")
  fi

  if [ -n "$prepare" ]; then
    hyperfine_args+=(--prepare "$prepare")
  fi

  write_line "### \`$label\`"
  if ! hyperfine "${hyperfine_args[@]}" --reference "$BASE_BINARY $cmd" "$HEAD_BINARY $cmd"; then
    write_line "⚠️ Benchmark failed for: $cmd"
    return 1
  fi
  write_benchmark_details
  if [ "$check_change" = "true" ]; then
    check_variance "$cmd"
  fi
}

create_meta_workspace() {
  rm -rf "$META_WORKSPACE"
  mkdir -p "$META_WORKSPACE"
  cd "$META_WORKSPACE"
  git init || { echo "Failed to init git for meta hooks"; exit 1; }
  git config user.name "Benchmark"
  git config user.email "bench@prek.dev"

  cp "$TARGET_WORKSPACE"/*.txt "$TARGET_WORKSPACE"/*.json . 2>/dev/null || true

  cat > .pre-commit-config.yaml << 'EOF'
repos:
  - repo: meta
    hooks:
      - id: check-hooks-apply
      - id: check-useless-excludes
      - id: identity
  - repo: builtin
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer
EOF

  git add -A
  git commit -m "Meta hooks test" || { echo "Failed to commit meta hooks test"; exit 1; }
  prek install-hooks
}

# Add environment metadata
write_line "## Hyperfine Performance"
write_blank_line
write_line "**Environment:**"
write_line "- OS: $(uname -s) $(uname -r)"
write_line "- CPU: $(nproc) cores"
write_line "- prek version: $CURRENT_PREK_VERSION"
write_line "- Rust version: $(rustc --version)"
write_line "- Hyperfine version: $(hyperfine --version)"

# Benchmark in the main repo
CMDS=(
  "--version"
  "list"
  "validate-config .pre-commit-config.yaml"
  "sample-config"
)
for cmd in "${CMDS[@]}"; do
  if [[ "$cmd" == "validate-config"* ]] && [ ! -f ".pre-commit-config.yaml" ]; then
    write_line "### \`prek $cmd\`"
    write_line "⏭️  Skipped: .pre-commit-config.yaml not found"
    continue
  fi

  if [[ "$cmd" == "--version" ]] || [[ "$cmd" == "list" ]]; then
    benchmark "prek $cmd" "$cmd" 5 100
  else
    benchmark "prek $cmd" "$cmd" 3 50
  fi
  check_variance "$cmd"
done

# Benchmark builtin hooks in test directory
cd "$TARGET_WORKSPACE"

# Cold vs warm benchmarks before polluting cache
write_section "Cold vs Warm Runs" "Comparing first run (cold) vs subsequent runs (warm cache):"
benchmark "prek run --all-files (cold - no cache)" "run --all-files" 0 10 "rm -rf ~/.cache/prek" "git checkout -- ."
benchmark "prek run --all-files (warm - with cache)" "run --all-files" 3 20 "" "git checkout -- ."

# Full benchmark suite with cache warmed up
write_section "Full Hook Suite" "Running the builtin hook suite on the benchmark workspace:"
benchmark "prek run --all-files (full builtin hook suite)" "run --all-files" 3 50 "" "git checkout -- ." true

# Individual hook performance
write_section "Individual Hook Performance" "Benchmarking each hook individually on the test repo:"

INDIVIDUAL_HOOKS=(
  "trailing-whitespace"
  "end-of-file-fixer"
  "check-json"
  "check-yaml"
  "check-toml"
  "check-xml"
)

for hook in "${INDIVIDUAL_HOOKS[@]}"; do
  benchmark "prek run $hook --all-files" "run $hook --all-files" 3 30 "" "git checkout -- ."
done

# Installation performance
write_section "Installation Performance" "Benchmarking hook installation (fast path hooks skip Python setup):"
benchmark "prek install-hooks (cold - no cache)" "install-hooks" 1 5 "rm -rf ~/.cache/prek/hooks ~/.cache/prek/repos"
benchmark "prek install-hooks (warm - with cache)" "install-hooks" 1 5

# File filtering/scoping performance
write_section "File Filtering/Scoping Performance" "Testing different file selection modes:"

git add -A
benchmark "prek run (staged files only)" "run" 3 20 "" "sh -c 'git checkout -- . && git add -A'"
benchmark "prek run --files '*.json' (specific file type)" "run --files '*.json'" 3 20

# Workspace discovery & initialization
write_section "Workspace Discovery & Initialization" "Benchmarking hook discovery and initialization overhead:"
benchmark "prek run --dry-run --all-files (measures init overhead)" "run --dry-run --all-files" 3 20

# Meta hooks performance
write_section "Meta Hooks Performance" "Benchmarking meta hooks separately:"
create_meta_workspace

META_HOOKS=(
  "check-hooks-apply"
  "check-useless-excludes"
  "identity"
)

for hook in "${META_HOOKS[@]}"; do
  benchmark "prek run $hook --all-files" "run $hook --all-files" 3 15 "" "git checkout -- ."
done

if [ "$failed" = true ]; then
  exit 1
fi
