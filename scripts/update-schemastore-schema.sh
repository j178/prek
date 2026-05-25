#!/usr/bin/env bash
set -exuo pipefail

repo_root=$(git rev-parse --show-toplevel)
schema="$repo_root/prek.schema.json"
schemastore="$(dirname "$repo_root")/schemastore"
current_tag=$(git -C "$repo_root" describe --tags --abbrev=0)
commit_message="Update prek schema to $current_tag"
target="src/schemas/json/prek.json"

if [[ ! -d "$schemastore/.git" ]]; then
  mkdir -p "$(dirname "$schemastore")"
  gh repo clone j178/schemastore "$schemastore" -- --depth=1
fi

(
  cd "$schemastore"

  git fetch upstream master
  git switch master
  git reset --hard upstream/master
  git push --force-with-lease origin master

  cp "$schema" "$target"
  prek run prettier --files "$target" || true
  if git diff --quiet -- "$target"; then
    echo "No changes to commit"
    exit 0
  fi
  git add "$target"
  git commit -m "$commit_message" -- "$target"
  git push origin master
)
