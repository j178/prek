#!/usr/bin/env bash
set -euo pipefail

MARKER='<!-- prek-hyperfine-benchmark -->'
RESULTS=$(cat /tmp/hyperfine-results/benchmark-results.md)
BODY="${MARKER}
${RESULTS}"
PR_NUMBER=$(cat /tmp/hyperfine-results/pr-number.txt)

if ! [[ "$PR_NUMBER" =~ ^[0-9]+$ ]]; then
  echo "Error: Invalid PR number: '$PR_NUMBER'"
  exit 1
fi

EXISTING_COMMENT_ID=$(
  gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
    --paginate --jq ".[] | select(.body | contains(\"${MARKER}\")) | .id"
)

if [ -n "$EXISTING_COMMENT_ID" ]; then
  echo "$BODY" | gh api "repos/${GITHUB_REPOSITORY}/issues/comments/${EXISTING_COMMENT_ID}" \
    --method PATCH --field body=@-
else
  echo "$BODY" | gh api "repos/${GITHUB_REPOSITORY}/issues/${PR_NUMBER}/comments" \
    --method POST --field body=@-
fi
