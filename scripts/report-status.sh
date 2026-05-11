#!/usr/bin/env bash
# Create or update a single rolling "Nightly build status" issue on the fork.
#
# Reads:
#   MERGED   (csv of PR numbers, may be empty)
#   SKIPPED  (csv of PR numbers, may be empty)
#   GITHUB_REPOSITORY, GITHUB_RUN_ID, GITHUB_SERVER_URL (set by Actions)
#
# Idempotent: an existing open issue with the configured title is edited in place;
# otherwise a new one is created.
set -euo pipefail

TITLE="${STATUS_ISSUE_TITLE:-Nightly build status}"
MERGED="${MERGED:-}"
SKIPPED="${SKIPPED:-}"
UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
UPSTREAM_BRANCH="${UPSTREAM_BRANCH:-master}"
UPSTREAM_REPO="${UPSTREAM_REPO:-helix-editor/helix}"

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be set}"
server="${GITHUB_SERVER_URL:-https://github.com}"
run_url="$server/$GITHUB_REPOSITORY/actions/runs/${GITHUB_RUN_ID:-}"

nightly_sha=$(git rev-parse HEAD)
upstream_sha=$(git rev-parse "$UPSTREAM_REMOTE/$UPSTREAM_BRANCH")
now=$(date -u +"%Y-%m-%d %H:%M:%S UTC")

format_list() {
  local csv="$1"
  if [[ -z "$csv" ]]; then
    echo "_none_"
    return
  fi
  IFS=',' read -r -a arr <<<"$csv"
  for n in "${arr[@]}"; do
    [[ -z "$n" ]] && continue
    echo "- ${UPSTREAM_REPO}#${n}"
  done
}

body=$(cat <<EOF
_Last updated: ${now}_

- **Workflow run:** ${run_url}
- **nightly:** \`${nightly_sha}\`
- **upstream/master:** \`${upstream_sha}\`

### Merged
$(format_list "$MERGED")

### Skipped (conflicts or lookup failure)
$(format_list "$SKIPPED")
EOF
)

existing=$(gh issue list \
  --repo "$GITHUB_REPOSITORY" \
  --state open \
  --search "$TITLE in:title" \
  --json number,title \
  --jq ".[] | select(.title == \"$TITLE\") | .number" | head -n1)

if [[ -n "$existing" ]]; then
  gh issue edit "$existing" --repo "$GITHUB_REPOSITORY" --body "$body"
else
  gh issue create --repo "$GITHUB_REPOSITORY" --title "$TITLE" --body "$body"
fi
