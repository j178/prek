#!/usr/bin/env bash
set -euo pipefail

TARGET_WORKSPACE=${HYPERFINE_BENCHMARK_WORKSPACE:?HYPERFINE_BENCHMARK_WORKSPACE is required}
COMMENT=${HYPERFINE_RESULTS_FILE:?HYPERFINE_RESULTS_FILE is required}
HEAD_BINARY=${HYPERFINE_HEAD_BINARY:?HYPERFINE_HEAD_BINARY is required}
BASE_BINARY=${HYPERFINE_BASE_BINARY:?HYPERFINE_BASE_BINARY is required}
OUT_DIR=$(dirname "$COMMENT")
META_WORKSPACE="${TARGET_WORKSPACE}-meta"

section_open=false
regression_count=0
improvement_count=0

mkdir -p "$OUT_DIR"
OUT_MD="$OUT_DIR/out.md"
OUT_JSON="$OUT_DIR/out.json"
REPORT_BODY="$OUT_DIR/report-body.md"

: > "$REPORT_BODY"

CURRENT_PREK_VERSION=$(
  "$HEAD_BINARY" --version | sed -n '1p'
)

write_line() {
  printf '%s\n' "$1" >> "$REPORT_BODY"
}

write_blank_line() {
  printf '\n' >> "$REPORT_BODY"
}

finalize_report() {
  : > "$COMMENT"
  printf '### ⚡️ Hyperfine Benchmarks\n\n' >> "$COMMENT"
  printf '**Summary:** %s regressions, %s improvements above the 10%% threshold.\n' "$regression_count" "$improvement_count" >> "$COMMENT"
  cat "$REPORT_BODY" >> "$COMMENT"
}

write_section() {
  local title="$1"
  local description="${2:-}"

  close_section
  write_blank_line
  write_line "<details>"
  write_line "<summary>$title</summary>"
  write_blank_line
  if [ -n "$description" ]; then
    write_line "$description"
    write_blank_line
  fi
  section_open=true
}

close_section() {
  if [ "$section_open" = true ]; then
    write_blank_line
    write_line "</details>"
    section_open=false
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
      improvement_count=$((improvement_count + 1))
      write_line "✅  Performance improvement for \`$cmd\`: ${pct#-}% faster"
    else
      regression_count=$((regression_count + 1))
      write_line "⚠️  Warning: Performance regression for \`$cmd\`: ${pct}% slower"
    fi
  fi
}

benchmark() {
  local cmd="$1"
  local warmup="${2:-3}"
  local runs="${3:-30}"
  local setup="${4:-}"
  local prepare="${5:-}"
  local check_change="${6:-false}"
  local label_suffix="${7:-}"
  local label="prek $cmd"
  local -a hyperfine_args=(-i -N -w "$warmup" -r "$runs" --export-markdown "$OUT_MD" --export-json "$OUT_JSON")

  if [ -n "$label_suffix" ]; then
    label="$label $label_suffix"
  fi

  if [ -n "$setup" ]; then
    hyperfine_args+=(--setup "$setup")
  fi

  if [ -n "$prepare" ]; then
    hyperfine_args+=(--prepare "$prepare")
  fi

  write_blank_line
  write_line "### \`$label\`"
  if ! hyperfine "${hyperfine_args[@]}" --reference "$BASE_BINARY $cmd" "$HEAD_BINARY $cmd"; then
    write_line "⚠️ Benchmark failed for: $cmd"
    return 1
  fi
  cat "$OUT_MD" >> "$REPORT_BODY"
  write_blank_line
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
  $HEAD_BINARY install-hooks
}

# Add environment metadata
write_line "<details>"
write_line "<summary>Environment</summary>"
write_blank_line
write_line "- OS: $(uname -s) $(uname -r)"
write_line "- CPU: $(nproc) cores"
write_line "- prek version: $CURRENT_PREK_VERSION"
write_line "- Rust version: $(rustc --version)"
write_line "- Hyperfine version: $(hyperfine --version)"
write_blank_line
write_line "</details>"

# Benchmark in the main repo
write_section "CLI Commands" "Benchmarking basic commands in the main repo:"

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
    benchmark "$cmd" 5 100
  else
    benchmark "$cmd" 3 50
  fi
  check_variance "$cmd"
done

# Benchmark builtin hooks in test directory
cd "$TARGET_WORKSPACE"

# Cold vs warm benchmarks before polluting cache
write_section "Cold vs Warm Runs" "Comparing first run (cold) vs subsequent runs (warm cache):"
benchmark "run --all-files" 0 10 "rm -rf ~/.cache/prek" "git checkout -- ." false "(cold - no cache)"
benchmark "run --all-files" 3 20 "" "git checkout -- ." false "(warm - with cache)"

# Full benchmark suite with cache warmed up
write_section "Full Hook Suite" "Running the builtin hook suite on the benchmark workspace:"
benchmark "run --all-files" 3 50 "" "git checkout -- ." true "(full builtin hook suite)"

# Individual hook performance
write_section "Individual Hook Performance" "Benchmarking each hook individually on the test repo:"

INDIVIDUAL_HOOKS=(
  "trailing-whitespace"
  "end-of-file-fixer"
  "check-json"
  "check-yaml"
  "check-toml"
  "check-xml"
  "detect-private-key"
  "fix-byte-order-marker"
)

for hook in "${INDIVIDUAL_HOOKS[@]}"; do
  benchmark "run $hook --all-files" 3 30 "" "git checkout -- ."
done

# Installation performance
write_section "Installation Performance" "Benchmarking hook installation (fast path hooks skip Python setup):"
benchmark "install-hooks" 1 5 "rm -rf ~/.cache/prek/hooks ~/.cache/prek/repos" "" false "(cold - no cache)"
benchmark "install-hooks" 1 5 "" "" false "(warm - with cache)"

# File filtering/scoping performance
write_section "File Filtering/Scoping Performance" "Testing different file selection modes:"

git add -A
benchmark "run" 3 20 "" "sh -c 'git checkout -- . && git add -A'" false "(staged files only)"
benchmark "run --files '*.json'" 3 20 "" "" false "(specific file type)"

# Workspace discovery & initialization
write_section "Workspace Discovery & Initialization" "Benchmarking hook discovery and initialization overhead:"
benchmark "run --dry-run --all-files" 3 20 "" "" false "(measures init overhead)"

# Meta hooks performance
write_section "Meta Hooks Performance" "Benchmarking meta hooks separately:"
create_meta_workspace

META_HOOKS=(
  "check-hooks-apply"
  "check-useless-excludes"
  "identity"
)

for hook in "${META_HOOKS[@]}"; do
  benchmark "run $hook --all-files" 3 15 "" "git checkout -- ."
done

close_section
finalize_report
