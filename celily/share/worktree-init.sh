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

# Without raw.idmap (no read-write host mount), the project belongs to an
# unmapped uid and git refuses it as "dubious ownership". Trust it, and the
# repositories under it, for this script and the command: appended to any
# GIT_CONFIG_COUNT entries already set, no file written.
n="${GIT_CONFIG_COUNT:-0}"
for dir in "${project}" "${project}/*"; do
    export "GIT_CONFIG_KEY_${n}=safe.directory" "GIT_CONFIG_VALUE_${n}=${dir}"
    n=$((n + 1))
done
export GIT_CONFIG_COUNT="${n}"

# show-ref exits 1 when the branch is missing. Any other failure (e.g.
# "dubious ownership") is an error: branching off HEAD instead would silently
# drop the existing branch.
status=0
git -C "${project}" show-ref --verify --quiet "refs/heads/${branch}" || status=$?
case "${status}" in
    0)
        git clone --quiet --shared --branch "${branch}" "${project}" "${worktree_path}"
        ;;
    1)
        git clone --quiet --shared "${project}" "${worktree_path}"
        git -C "${worktree_path}" switch --quiet --create "${branch}"
        ;;
    *)
        printf 'worktree-init: cannot look up branch %s in %s\n' "${branch}" "${project}" >&2
        exit "${status}"
        ;;
esac

cd "${worktree_path}"
exec "$@"
