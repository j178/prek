#!/usr/bin/env bash
set -euo pipefail

MARKER='<!-- prek-hyperfine-benchmark -->'
RESULTS=$(cat /tmp/hyperfine-results/benchmark-results.md)
BODY="${MARKER}
${RESULTS}"
PR_NUMBER=$(cat /tmp/hyperfine-results/pr-number.txt)

EXISTING_COMMENT_ID=$(
  gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
    --paginate --jq ".[] | select(.body | contains(\"${MARKER}\")) | .id"
)

if [ -n "$EXISTING_COMMENT_ID" ]; then
  gh api "repos/${GITHUB_REPOSITORY}/issues/comments/${EXISTING_COMMENT_ID}" \
    --method PATCH --field body="$BODY"
else
  gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
    --method POST --field body="$BODY"
fi
