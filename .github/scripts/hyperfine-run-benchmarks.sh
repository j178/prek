#!/usr/bin/env bash
set -euo pipefail
set -x

failed=false

# Add prek binaries to PATH
export PATH="$HOME/bin:/home/runner/bin:$PATH"

run_hyperfine() {
  local cmd="$1"
  local warmup="${2:-3}"
  local runs="${3:-30}"
  local setup="${4:-}"
  local prepare="${5:-}"

  # -i flag: ignore non-zero exit codes (hooks return 1 when they modify files)
  local -a hyperfine_args=(-i -N -w "$warmup" -r "$runs" --export-markdown out.md --export-json out.json --show-output)

  if [ -n "$setup" ]; then
    hyperfine_args+=(--setup "$setup")
  fi

  if [ -n "$prepare" ]; then
    hyperfine_args+=(--prepare "$prepare")
  fi

  if [ -n "${PREK_ALT:-}" ]; then
    if ! hyperfine "${hyperfine_args[@]}" --reference "$PREK_ALT $cmd" "prek $cmd"; then
      echo "⚠️ Benchmark failed for: $cmd" >> "$GITHUB_WORKSPACE/comment.md"
      return 1
    fi
  elif [ -f "$HOME/bin/prek-$MAIN_VERSION" ]; then
    if ! hyperfine "${hyperfine_args[@]}" --reference "prek-$MAIN_VERSION $cmd" "prek $cmd"; then
      echo "⚠️ Benchmark failed for: $cmd" >> "$GITHUB_WORKSPACE/comment.md"
      return 1
    fi
  else
    if ! hyperfine "${hyperfine_args[@]}" "prek $cmd"; then
      echo "⚠️ Benchmark failed for: $cmd" >> "$GITHUB_WORKSPACE/comment.md"
      return 1
    fi
  fi
}

output_results() {
  cat out.md >> "$GITHUB_WORKSPACE/comment.md"
  cat out.md
}

check_variance() {
  local cmd="$1"
  if grep -q "±.*±" out.md; then
    local variance
    variance=$(grep "±.*±" out.md | awk '{print $(NF-3)}' | sed 's/%//' | head -1)
    if [ -n "$variance" ]; then
      local diff
      diff=$(echo "scale=2; ($variance - 1) * 100" | bc)

      if (( $(echo "${diff#-} > 10" | bc -l) )); then
        if grep -q "prek-$MAIN_VERSION.*±.*±" out.md; then
          echo "✅  Performance improvement for \`$cmd\` is ${diff#-}%" >> "$GITHUB_WORKSPACE/comment.md"
        else
          echo "⚠️  Warning: Performance regression for \`$cmd\` is ${diff#-}%" >> "$GITHUB_WORKSPACE/comment.md"
          failed=true
        fi
      fi
    fi
  fi
}

# Add environment metadata
echo "## Hyperfine Performance" > "$GITHUB_WORKSPACE/comment.md"
echo "" >> "$GITHUB_WORKSPACE/comment.md"
echo "**Environment:**" >> "$GITHUB_WORKSPACE/comment.md"
echo "- OS: $(uname -s) $(uname -r)" >> "$GITHUB_WORKSPACE/comment.md"
echo "- CPU: $(nproc) cores" >> "$GITHUB_WORKSPACE/comment.md"
echo "- prek version: $(prek --version)" >> "$GITHUB_WORKSPACE/comment.md"
echo "- Rust version: $(rustc --version)" >> "$GITHUB_WORKSPACE/comment.md"
echo "- Hyperfine version: $(hyperfine --version)" >> "$GITHUB_WORKSPACE/comment.md"
echo "" >> "$GITHUB_WORKSPACE/comment.md"

# Benchmark in the main repo
CMDS=(
  "--version"
  "list"
  "validate-config .pre-commit-config.yaml"
  "sample-config"
)
for cmd in "${CMDS[@]}"; do
  if [[ "$cmd" == "validate-config"* ]] && [ ! -f ".pre-commit-config.yaml" ]; then
    echo "### \`prek $cmd\`" >> "$GITHUB_WORKSPACE/comment.md"
    echo "⏭️  Skipped: .pre-commit-config.yaml not found" >> "$GITHUB_WORKSPACE/comment.md"
    continue
  fi

  echo "### \`prek $cmd\`" >> "$GITHUB_WORKSPACE/comment.md"
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
echo "" >> "$GITHUB_WORKSPACE/comment.md"
echo "## Cold vs Warm Runs" >> "$GITHUB_WORKSPACE/comment.md"
echo "Comparing first run (cold) vs subsequent runs (warm cache):" >> "$GITHUB_WORKSPACE/comment.md"

echo "### \`prek run --all-files\` (cold - no cache)" >> "$GITHUB_WORKSPACE/comment.md"
run_hyperfine "run --all-files" 0 10 "rm -rf ~/.cache/prek" "git checkout -- ."
output_results

echo "### \`prek run --all-files\` (warm - with cache)" >> "$GITHUB_WORKSPACE/comment.md"
run_hyperfine "run --all-files" 3 20 "" "git checkout -- ."
output_results

# Full benchmark suite with cache warmed up
echo "" >> "$GITHUB_WORKSPACE/comment.md"
echo "## Full Hook Suite" >> "$GITHUB_WORKSPACE/comment.md"
echo "Running all 13 builtin hooks on 260 test files:" >> "$GITHUB_WORKSPACE/comment.md"

echo "### \`prek run --all-files\` (13 builtin hooks on 260 files)" >> "$GITHUB_WORKSPACE/comment.md"
run_hyperfine "run --all-files" 3 50 "" "git checkout -- ."
output_results
check_variance "run --all-files"

cd "$GITHUB_WORKSPACE"

# Individual hook performance
echo "" >> "$GITHUB_WORKSPACE/comment.md"
echo "## Individual Hook Performance" >> "$GITHUB_WORKSPACE/comment.md"
echo "Benchmarking each hook individually on the test repo:" >> "$GITHUB_WORKSPACE/comment.md"

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
  echo "### \`prek run $hook --all-files\`" >> "$GITHUB_WORKSPACE/comment.md"
  run_hyperfine "run $hook --all-files" 3 30 "" "git checkout -- ."
  output_results
done

# Installation performance
echo "" >> "$GITHUB_WORKSPACE/comment.md"
echo "## Installation Performance" >> "$GITHUB_WORKSPACE/comment.md"
echo "Benchmarking hook installation (fast path hooks skip Python setup):" >> "$GITHUB_WORKSPACE/comment.md"

echo "### \`prek install-hooks\` (cold - no cache)" >> "$GITHUB_WORKSPACE/comment.md"
run_hyperfine "install-hooks" 1 5 "rm -rf ~/.cache/prek/hooks ~/.cache/prek/repos"
output_results

echo "### \`prek install-hooks\` (warm - with cache)" >> "$GITHUB_WORKSPACE/comment.md"
run_hyperfine "install-hooks" 1 5
output_results

# File filtering/scoping performance
echo "" >> "$GITHUB_WORKSPACE/comment.md"
echo "## File Filtering/Scoping Performance" >> "$GITHUB_WORKSPACE/comment.md"
echo "Testing different file selection modes:" >> "$GITHUB_WORKSPACE/comment.md"

cd /tmp/prek-bench

git add -A
echo "### \`prek run\` (staged files only)" >> "$GITHUB_WORKSPACE/comment.md"
run_hyperfine "run" 3 20 "" "sh -c 'git checkout -- . && git add -A'"
output_results

echo "### \`prek run --files '*.json'\` (specific file type)" >> "$GITHUB_WORKSPACE/comment.md"
run_hyperfine "run --files '*.json'" 3 20
output_results

# Workspace discovery & initialization
echo "" >> "$GITHUB_WORKSPACE/comment.md"
echo "## Workspace Discovery & Initialization" >> "$GITHUB_WORKSPACE/comment.md"
echo "Benchmarking hook discovery and initialization overhead:" >> "$GITHUB_WORKSPACE/comment.md"

cd /tmp/prek-bench
echo "### \`prek run --dry-run --all-files\` (measures init overhead)" >> "$GITHUB_WORKSPACE/comment.md"
run_hyperfine "run --dry-run --all-files" 3 20
output_results

# Meta hooks performance
echo "" >> "$GITHUB_WORKSPACE/comment.md"
echo "## Meta Hooks Performance" >> "$GITHUB_WORKSPACE/comment.md"
echo "Benchmarking meta hooks separately:" >> "$GITHUB_WORKSPACE/comment.md"

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
  echo "### \`prek run $hook --all-files\`" >> "$GITHUB_WORKSPACE/comment.md"
  run_hyperfine "run $hook --all-files" 3 15 "" "git checkout -- ."
  output_results
done

cd "$GITHUB_WORKSPACE"

if [ "$failed" = true ]; then
  exit 1
fi
