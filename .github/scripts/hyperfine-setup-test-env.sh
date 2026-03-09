#!/usr/bin/env bash
set -euo pipefail

TARGET_WORKSPACE=${HYPERFINE_BENCHMARK_WORKSPACE:?HYPERFINE_BENCHMARK_WORKSPACE is required}

# Create a clean test directory with files to run builtin hooks against
rm -rf "$TARGET_WORKSPACE"
mkdir -p "$TARGET_WORKSPACE"
cd "$TARGET_WORKSPACE"
git init || { echo "Failed to init git"; exit 1; }
git config user.name "Benchmark"
git config user.email "bench@prek.dev"

# Files with trailing whitespace and no final newline
for i in {1..50}; do
  printf "line with trailing whitespace   \nanother line  " > "file$i.txt"
done

# JSON files
for i in {1..30}; do
  echo '{"key": "value", "number": '$i'}' > "file$i.json"
done

# YAML files
for i in {1..30}; do
  echo "key: value" > "file$i.yaml"
  echo "number: $i" >> "file$i.yaml"
done

# TOML files
for i in {1..30}; do
  echo "[section]" > "file$i.toml"
  echo "key = \"value$i\"" >> "file$i.toml"
done

# XML files
for i in {1..30}; do
  echo '<?xml version="1.0"?><root><item id="'$i'">value</item></root>' > "file$i.xml"
done

# Files with mixed line endings
for i in {1..20}; do
  printf "line1\r\nline2\nline3\r\n" > "mixed$i.txt"
done

# Files with UTF-8 BOM
for i in {1..20}; do
  printf '\xef\xbb\xbfContent with BOM' > "bom$i.txt"
done

# Executable files (for shebang check)
for i in {1..10}; do
  echo "#!/bin/bash" > "script$i.sh"
  echo "echo hello" >> "script$i.sh"
  chmod +x "script$i.sh"
done

# Files that might contain private keys (but don't)
for i in {1..10}; do
  echo "# This is not a private key" > "config$i.txt"
  echo "api_key = fake_key_$i" >> "config$i.txt"
done

# Create symlinks for check-symlinks
for i in {1..10}; do
  ln -s "file$i.txt" "link$i.txt"
done

# Create a config that uses all builtin hooks
cat > .pre-commit-config.yaml << 'EOF'
repos:
  - repo: builtin
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer
      - id: check-json
      - id: check-yaml
      - id: check-toml
      - id: check-xml
      - id: mixed-line-ending
      - id: fix-byte-order-marker
      - id: check-executables-have-shebangs
      - id: detect-private-key
      - id: check-case-conflict
      - id: check-merge-conflict
      - id: check-symlinks
EOF

git add -A
git commit -m "Initial commit" || { echo "Failed to commit"; exit 1; }
