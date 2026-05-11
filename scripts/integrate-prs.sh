#!/usr/bin/env bash
# Build the `nightly` branch:
#   upstream/master + each enabled PR from prs.yml merged with `git merge --no-ff`.
#
# On a per-PR conflict the script aborts the merge, records the PR number in the
# skipped list, and continues. The script itself only exits non-zero on unrelated
# infrastructure failures (missing yq/jq/gh, unreachable upstream, etc.).
#
# Emits two outputs to $GITHUB_OUTPUT (comma-separated):
#   merged=<n>,<n>,...
#   skipped=<n>,<n>,...
set -euo pipefail

PRS_FILE="${PRS_FILE:-prs.yml}"
UPSTREAM_REMOTE="${UPSTREAM_REMOTE:-upstream}"
UPSTREAM_BRANCH="${UPSTREAM_BRANCH:-master}"
NIGHTLY_BRANCH="${NIGHTLY_BRANCH:-nightly}"
UPSTREAM_REPO="${UPSTREAM_REPO:-helix-editor/helix}"

for cmd in yq jq gh git; do
  command -v "$cmd" >/dev/null 2>&1 || { echo "::error::missing required tool: $cmd"; exit 1; }
done

merged=()
skipped=()

git checkout -B "$NIGHTLY_BRANCH" "$UPSTREAM_REMOTE/$UPSTREAM_BRANCH"

prs_json=$(yq -o=json '.prs' "$PRS_FILE")
count=$(jq 'length' <<<"$prs_json")

for ((i = 0; i < count; i++)); do
  entry=$(jq ".[$i]" <<<"$prs_json")
  number=$(jq -r '.number'      <<<"$entry")
  enabled=$(jq -r '.enabled // true' <<<"$entry")
  pin=$(jq -r '.pin // ""'      <<<"$entry")

  if [[ "$enabled" != "true" ]]; then
    echo "::notice::PR #$number disabled in $PRS_FILE — skipping"
    continue
  fi

  if ! meta=$(gh pr view "$number" --repo "$UPSTREAM_REPO" \
                --json number,title,state,headRefOid 2>/dev/null); then
    echo "::warning::could not look up PR #$number — skipping"
    skipped+=("$number")
    continue
  fi

  state=$(jq -r '.state' <<<"$meta")
  title=$(jq -r '.title' <<<"$meta")
  if [[ "$state" == "CLOSED" ]]; then
    echo "::notice::PR #$number is CLOSED — skipping"
    continue
  fi

  sha="${pin:-$(jq -r '.headRefOid' <<<"$meta")}"

  echo "::group::PR #$number — $title"
  echo "head: $sha"
  git fetch "$UPSTREAM_REMOTE" "pull/$number/head:refs/nightly/pr-$number" --no-tags

  if git merge --no-ff -m "Integrate PR #$number: $title" "$sha"; then
    merged+=("$number")
    echo "merged PR #$number"
  else
    echo "::warning::conflict merging PR #$number — skipping"
    git merge --abort || true
    skipped+=("$number")
  fi
  echo "::endgroup::"
done

merged_csv=$(IFS=,; echo "${merged[*]:-}")
skipped_csv=$(IFS=,; echo "${skipped[*]:-}")

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  {
    echo "merged=$merged_csv"
    echo "skipped=$skipped_csv"
  } >> "$GITHUB_OUTPUT"
fi

echo "Merged:  ${merged[*]:-(none)}"
echo "Skipped: ${skipped[*]:-(none)}"
