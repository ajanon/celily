#!/bin/bash -eu
# rbw stub, installed as `rbw` first in celily's PATH. Answers the two calls
# celily makes, `rbw unlocked` then `rbw get ITEM`, and fails on anything else.
# test_network.py expects `secret` for `item`.

declare -r item="foo"
declare -r secret="celily-it-secret-5c2e9a"

main() {
    case "$*" in
    unlocked) ;;
    "get ${item}")
        printf '%s\n' "${secret}"
        ;;
    *)
        printf 'rbw stub: unexpected arguments: %s\n' "$*" >&2
        exit 1
        ;;
    esac
}

main "$@"
