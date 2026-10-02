#!/bin/sh -eu
#
# Worktree initialization script for celily.
#
# The project, .git included, is mounted read-only. Clone it into
# $HOME/<name>, borrowing its objects (--shared) so nothing is copied. New
# commits stay in the clone; the host fetches them through its own remote.
#
#   CELILY_WORKTREE_BRANCH       branch name for the worktree
#   CELILY_WORKTREE_NAME         worktree directory name (under $HOME)
#   CELILY_WORKTREE_PROJECT      path to the read-only project checkout

# celily runs this through `sh -c`, so the flags on the shebang line do not
# apply. Without -e, a failed clone would leave the command running in the
# read-only project, or on the wrong branch.
set -eu

worktree_path="${HOME}/${CELILY_WORKTREE_NAME}"
project="${CELILY_WORKTREE_PROJECT}"
branch="${CELILY_WORKTREE_BRANCH}"

if git -C "${project}" show-ref --verify --quiet "refs/heads/${branch}"; then
    git clone --quiet --shared --branch "${branch}" "${project}" "${worktree_path}"
else
    git clone --quiet --shared "${project}" "${worktree_path}"
    git -C "${worktree_path}" switch --quiet --create "${branch}"
fi

cd "${worktree_path}"
exec "$@"
