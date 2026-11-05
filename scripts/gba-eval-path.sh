#!/usr/bin/env bash
# Print the default gba-eval location: ../../gba-eval relative to this repo.
# The main checkout is used even when called from a git worktree (worktrees live
# under .claude/worktrees/, where a plain relative path would be wrong).
set -euo pipefail
common_dir="$(git -C "$(dirname "$0")" rev-parse --path-format=absolute --git-common-dir)"
repo_root="$(dirname "$common_dir")"
cd "$repo_root/../../gba-eval" 2>/dev/null && pwd || { echo "$repo_root/../../gba-eval"; }
