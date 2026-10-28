#!/bin/sh
set -eu
subject=$(sed -n '/^[^#]/ {p;q;}' "$1")
if ! printf '%s\n' "$subject" |
    grep -qE '^(feat|fix|docs|chore|refactor|perf|test|build|ci|style|revert)\([a-z0-9][a-z0-9_./-]*\)!?: [^ ]'; then
    echo 'Commit subject must follow type(scope): summary.' >&2
    exit 1
fi
if [ "$(printf '%s' "$subject" | wc -m)" -gt 72 ]; then
    echo 'Commit subject must be at most 72 characters.' >&2
    exit 1
fi
