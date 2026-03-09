#!/usr/bin/env bash
set -euo pipefail
set -x

failed=false

COMMENT="$GITHUB_WORKSPACE/comment.md"
OUT_MD="$GITHUB_WORKSPACE/out.md"
OUT_JSON="$GITHUB_WORKSPACE/out.json"

# Add prek binaries to PATH
export PATH="$HOME/bin:/home/runner/bin:$PATH"

run_hyperfine() {
  local cmd="$1"
  local warmup="${2:-3}"
  local runs="${3:-30}"
  local setup="${4:-}"
  local prepare="${5:-}"

  # -i flag: ignore non-zero exit codes (hooks return 1 when they modify files)
  local -a hyperfine_args=(-i -N -w "$warmup" -r "$runs" --export-markdown "$OUT_MD" --export-json "$OUT_JSON" --show-output)

  if [ -n "$setup" ]; then
    hyperfine_args+=(--setup "$setup")
  fi

  if [ -n "$prepare" ]; then
    hyperfine_args+=(--prepare "$prepare")
  fi

  if [ -n "${PREK_ALT:-}" ]; then
    if ! hyperfine "${hyperfine_args[@]}" --reference "$PREK_ALT $cmd" "prek $cmd"; then
      echo "⚠️ Benchmark failed for: $cmd" >> "$COMMENT"
      return 1
    fi
  elif [ -f "$HOME/bin/prek-$MAIN_VERSION" ]; then
    if ! hyperfine "${hyperfine_args[@]}" --reference "prek-$MAIN_VERSION $cmd" "prek $cmd"; then
      echo "⚠️ Benchmark failed for: $cmd" >> "$COMMENT"
      return 1
    fi
  else
    if ! hyperfine "${hyperfine_args[@]}" "prek $cmd"; then
      echo "⚠️ Benchmark failed for: $cmd" >> "$COMMENT"
      return 1
    fi
  fi
}

output_results() {
  cat "$OUT_MD" >> "$COMMENT"
  cat "$OUT_MD"
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
      echo "✅  Performance improvement for \`$cmd\`: ${pct#-}% faster" >> "$COMMENT"
    else
      echo "⚠️  Warning: Performance regression for \`$cmd\`: ${pct}% slower" >> "$COMMENT"
      failed=true
    fi
  fi
}

# Add environment metadata
echo "## Hyperfine Performance" > "$COMMENT"
echo "" >> "$COMMENT"
echo "**Environment:**" >> "$COMMENT"
echo "- OS: $(uname -s) $(uname -r)" >> "$COMMENT"
echo "- CPU: $(nproc) cores" >> "$COMMENT"
echo "- prek version: $(prek --version)" >> "$COMMENT"
echo "- Rust version: $(rustc --version)" >> "$COMMENT"
echo "- Hyperfine version: $(hyperfine --version)" >> "$COMMENT"
echo "" >> "$COMMENT"

# Benchmark in the main repo
CMDS=(
  "--version"
  "list"
  "validate-config .pre-commit-config.yaml"
  "sample-config"
)
for cmd in "${CMDS[@]}"; do
  if [[ "$cmd" == "validate-config"* ]] && [ ! -f ".pre-commit-config.yaml" ]; then
    echo "### \`prek $cmd\`" >> "$COMMENT"
    echo "⏭️  Skipped: .pre-commit-config.yaml not found" >> "$COMMENT"
    continue
  fi

  echo "### \`prek $cmd\`" >> "$COMMENT"
  if [[ "$cmd" == "--version" ]] || [[ "$cmd" == "list" ]]; then
    run_hyperfine "$cmd" 5 100
  else
    run_hyperfine "$cmd" 3 50
  fi
  output_results
  check_variance "$cmd"
done

# Benchmark builtin hooks in test directory
cd /tmp/prek-bench

# Cold vs warm benchmarks before polluting cache
echo "" >> "$COMMENT"
echo "## Cold vs Warm Runs" >> "$COMMENT"
echo "Comparing first run (cold) vs subsequent runs (warm cache):" >> "$COMMENT"

echo "### \`prek run --all-files\` (cold - no cache)" >> "$COMMENT"
run_hyperfine "run --all-files" 0 10 "rm -rf ~/.cache/prek" "git checkout -- ."
output_results

echo "### \`prek run --all-files\` (warm - with cache)" >> "$COMMENT"
run_hyperfine "run --all-files" 3 20 "" "git checkout -- ."
output_results

# Full benchmark suite with cache warmed up
echo "" >> "$COMMENT"
echo "## Full Hook Suite" >> "$COMMENT"
echo "Running all 13 builtin hooks on 260 test files:" >> "$COMMENT"

echo "### \`prek run --all-files\` (13 builtin hooks on 260 files)" >> "$COMMENT"
run_hyperfine "run --all-files" 3 50 "" "git checkout -- ."
output_results
check_variance "run --all-files"

cd "$GITHUB_WORKSPACE"

# Individual hook performance
echo "" >> "$COMMENT"
echo "## Individual Hook Performance" >> "$COMMENT"
echo "Benchmarking each hook individually on the test repo:" >> "$COMMENT"

cd /tmp/prek-bench
INDIVIDUAL_HOOKS=(
  "trailing-whitespace"
  "end-of-file-fixer"
  "check-json"
  "check-yaml"
  "check-toml"
  "check-xml"
)

for hook in "${INDIVIDUAL_HOOKS[@]}"; do
  echo "### \`prek run $hook --all-files\`" >> "$COMMENT"
  run_hyperfine "run $hook --all-files" 3 30 "" "git checkout -- ."
  output_results
done

# Installation performance
echo "" >> "$COMMENT"
echo "## Installation Performance" >> "$COMMENT"
echo "Benchmarking hook installation (fast path hooks skip Python setup):" >> "$COMMENT"

echo "### \`prek install-hooks\` (cold - no cache)" >> "$COMMENT"
run_hyperfine "install-hooks" 1 5 "rm -rf ~/.cache/prek/hooks ~/.cache/prek/repos"
output_results

echo "### \`prek install-hooks\` (warm - with cache)" >> "$COMMENT"
run_hyperfine "install-hooks" 1 5
output_results

# File filtering/scoping performance
echo "" >> "$COMMENT"
echo "## File Filtering/Scoping Performance" >> "$COMMENT"
echo "Testing different file selection modes:" >> "$COMMENT"

cd /tmp/prek-bench

git add -A
echo "### \`prek run\` (staged files only)" >> "$COMMENT"
run_hyperfine "run" 3 20 "" "sh -c 'git checkout -- . && git add -A'"
output_results

echo "### \`prek run --files '*.json'\` (specific file type)" >> "$COMMENT"
run_hyperfine "run --files '*.json'" 3 20
output_results

# Workspace discovery & initialization
echo "" >> "$COMMENT"
echo "## Workspace Discovery & Initialization" >> "$COMMENT"
echo "Benchmarking hook discovery and initialization overhead:" >> "$COMMENT"

cd /tmp/prek-bench
echo "### \`prek run --dry-run --all-files\` (measures init overhead)" >> "$COMMENT"
run_hyperfine "run --dry-run --all-files" 3 20
output_results

# Meta hooks performance
echo "" >> "$COMMENT"
echo "## Meta Hooks Performance" >> "$COMMENT"
echo "Benchmarking meta hooks separately:" >> "$COMMENT"

rm -rf /tmp/prek-bench-meta
mkdir -p /tmp/prek-bench-meta
cd /tmp/prek-bench-meta
git init || { echo "Failed to init git for meta hooks"; exit 1; }
git config user.name "Benchmark"
git config user.email "bench@prek.dev"

cp /tmp/prek-bench/*.txt /tmp/prek-bench/*.json . 2>/dev/null || true

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

META_HOOKS=(
  "check-hooks-apply"
  "check-useless-excludes"
  "identity"
)

for hook in "${META_HOOKS[@]}"; do
  echo "### \`prek run $hook --all-files\`" >> "$COMMENT"
  run_hyperfine "run $hook --all-files" 3 15 "" "git checkout -- ."
  output_results
done

cd "$GITHUB_WORKSPACE"

if [ "$failed" = true ]; then
  exit 1
fi
