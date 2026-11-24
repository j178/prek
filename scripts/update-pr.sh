#!/usr/bin/env bash
# Publish the staged changes from main. Extra arguments are passed to gh pr create.
set -euo pipefail

pr=$(gh pr list --base main --head "$PR_BRANCH" --json url,isCrossRepository \
    --jq '.[] | select(.isCrossRepository == false) | .url')
if git diff --cached --quiet; then
    if [[ -n "$pr" ]]; then
        gh pr close "$pr"
    fi
    exit 0
fi

remote_ref="refs/remotes/origin/$PR_BRANCH"
# Reuse the published commit if it already has the generated tree on the current main.
if ! git merge-base --is-ancestor HEAD "$remote_ref" 2>/dev/null || ! git diff --cached --quiet "$remote_ref"; then
    git config user.name 'github-actions[bot]'
    git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
    git commit -m "$PR_TITLE"
    gh auth setup-git
    git push --force-with-lease origin "HEAD:refs/heads/$PR_BRANCH"
fi

if [[ -z "$pr" ]]; then
    pr=$(gh pr create --base main --head "$PR_BRANCH" --title "$PR_TITLE" --body-file - "$@" <<< "$PR_BODY")
else
    gh pr edit "$pr" --title "$PR_TITLE" --body-file - <<< "$PR_BODY"
fi
echo "url=$pr" >> "$GITHUB_OUTPUT"
