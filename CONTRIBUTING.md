# Contributing

Read [the overview](README.md) and the Rust API documentation before changing
public interfaces. Keep the README and API documentation consistent with the
implementation and its limits.

## Setup

Rustup selects the toolchain pinned in `rust-toolchain.toml`. Once the
directory is a Git repository, install the optional commit-message hook:

```sh
make hooks
```

## Gates

`make all` runs formatting, Clippy with warnings denied, tests, rustdoc with
warnings denied, cargo-deny, the core dependency allow list and each
executable example. Install cargo-deny with `cargo install cargo-deny --locked`.
The gate fails rather than silently skipping an unavailable checker.

Run `make msrv` when adding dependencies or using a newer standard-library
API. `make msrv-toolchain` installs the workspace's minimum version.
CI performs the same Rust checks and audits dependencies weekly.

```sh
make fmt
make all
make msrv
```

In an offline environment with a populated Cargo cache and advisory database,
`CARGO_NET_OFFLINE=true make DENY_FLAGS=--offline all` uses the local data.
That checks the cached advisories only; CI refreshes them online.

The optional Rust adapter lives in the separate `adapters/roomkit` workspace.
With `roomkit-rs` beside this checkout, run `make roomkit-check roomkit-msrv`.
These explicit gates are not part of the standalone `make all` or CI, which
must work without a RoomKit checkout. The adapter currently uses local path
dependencies; it requires a compatible sibling checkout and is not distributed
as a published package.
The adapter owns its dependency versions and lockfile while it is separate.

## Code and dependencies

- Code, comments, examples and developer documentation use English. Keep
  internal planning, local integration assets and personal data out of commits.
- Public items are documented. Library code returns typed failures rather
  than panicking on caller input.
- Prefer `#[expect(lint, reason = "...")]` for a necessary exception.
- `roomkit-tools-core` contains contracts and pure logic only. No filesystem,
  processes, sockets, global configuration or async runtime belongs there.
- Add dependencies to `workspace.dependencies`, then opt into the features
  needed by each consuming crate. Document their purpose near the declaration.
- A core dependency must also be reflected in `scripts/check-core-deps.sh`.
- Keep RoomKit, Python and provider-specific types out of the core and tools.
- Keep cancellation, resource ownership and result limits explicit.

## Tests and examples

Test observable behavior, especially boundaries, side effects and cleanup.
Filesystem tests use temporary directories; they never modify a developer's
checkout. Tests and examples require no external service or credentials.

Each `examples/*.rs` file is declared in `examples/Cargo.toml`, checks its
result and returns a failing exit code when its behavior changes.

## Commits

Use a subject of at most 72 characters: `type(scope): summary`, for example
`feat(fs): add paginated text reading`. The optional hook and CI use the
same checker. Add user-visible changes under `Unreleased` in `CHANGELOG.md`.
