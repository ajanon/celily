#!/bin/sh -eu
#
# Worktree initialization script for celily.
#
#   CELILY_WORKTREE_BRANCH       branch name for the worktree
#   CELILY_WORKTREE_NAME         worktree directory name (under $HOME)
#   CELILY_WORKTREE_PROJECT      path to the main project checkout
#   CELILY_WORKTREE_INSTANCE     instance name (for auto-commit message)
#   CELILY_WORKTREE_AUTO_COMMIT  "1" to auto-commit changes, "0" to skip

worktree_path="${HOME}/${CELILY_WORKTREE_NAME}"

cd "${CELILY_WORKTREE_PROJECT}"

if git show-ref --verify --quiet "refs/heads/${CELILY_WORKTREE_BRANCH}"; then
    git worktree add "${worktree_path}" "${CELILY_WORKTREE_BRANCH}"
else
    git worktree add "${worktree_path}" -b "${CELILY_WORKTREE_BRANCH}" HEAD
fi

cd "${worktree_path}"

(
    "$@"
)
rc=$?

if [ "${CELILY_WORKTREE_AUTO_COMMIT}" = "1" ]; then
    if git status --porcelain | grep -q .; then
        git add -A
        git commit --no-verify -m "celily: auto-commit ${CELILY_WORKTREE_INSTANCE}"
    fi
fi

# Remove worktree metadata so the main project's .git is clean.
# The branch and all commits survive.
cd "${CELILY_WORKTREE_PROJECT}"
git worktree remove --force "${worktree_path}"

exit "${rc}"
