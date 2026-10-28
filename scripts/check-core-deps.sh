#!/usr/bin/env bash
# Keep the contracts usable without an async runtime, Python or RoomKit.
set -euo pipefail

allowed=(
    roomkit-tools-core
    serde serde_core serde_derive serde_json itoa memchr zmij
    schemars schemars_derive dyn-clone ref-cast ref-cast-impl serde_derive_internals
    thiserror thiserror-impl
    proc-macro2 quote syn unicode-ident
)

dependencies=$(
    "${CARGO:-cargo}" tree --package roomkit-tools-core --edges normal,build \
        --prefix none --format '{p}' --locked --target all --all-features |
        awk '{print $1}' | LC_ALL=C sort -u
)
unexpected=$(LC_ALL=C comm -23 \
    <(printf '%s\n' "$dependencies") \
    <(printf '%s\n' "${allowed[@]}" | LC_ALL=C sort -u))
if [[ -n "$unexpected" ]]; then
    printf 'Unexpected roomkit-tools-core dependencies:\n%s\n' "$unexpected" >&2
    exit 1
fi
echo 'roomkit-tools-core dependencies: contracts only'
