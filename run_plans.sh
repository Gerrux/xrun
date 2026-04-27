#!/usr/bin/env bash
# Runs all xrun plans sequentially. After each plan completes,
# merges the feature branch back to master so the next plan builds on it.
set -e
cd "$(dirname "$0")"

PLANS=(
  "docs/plans/2026-04-27-xrun-v0.1-foundation.md"
  "docs/plans/2026-04-27-xrun-v0.1-bis-vast-poller-hook.md"
  "docs/plans/2026-04-27-xrun-v0.2-tui.md"
  "docs/plans/2026-04-27-xrun-v0.3-mlflow-kaggle.md"
)

run_plan() {
  local plan_file="$1"
  local basename
  basename=$(basename "$plan_file" .md)
  # ralphex strips the YYYY-MM-DD- date prefix when creating the branch
  local branch="${basename#2026-04-27-}"

  echo ""
  echo "========================================"
  echo "STARTING: $plan_file"
  echo "BRANCH:   $branch"
  echo "========================================"

  ralphex --serve "$plan_file"

  echo ""
  echo "========================================"
  echo "COMPLETED: $plan_file"
  echo "Merging $branch → master"
  echo "========================================"

  git checkout master
  git merge "$branch" --no-ff -m "merge: $branch completed"

  echo "Merged OK. Continuing to next plan."
}

for plan in "${PLANS[@]}"; do
  # Skip plans already moved to completed/
  if [ ! -f "$plan" ]; then
    echo "SKIP (already completed): $plan"
    continue
  fi
  run_plan "$plan"
done

echo ""
echo "========================================"
echo "ALL PLANS DONE"
echo "========================================"
