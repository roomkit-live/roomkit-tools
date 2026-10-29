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

The optional Python bridge is another separate workspace, `bindings/python`.
With CPython 3.12+, uv and cargo-deny installed, run `make python-check
python-msrv`. The check builds a wheel, installs it in a temporary environment
without Python dependencies, runs the standalone tests, then installs the
minimum supported RoomKit release for adapter tests. CI runs on Python 3.12 and
3.14. Use `uv tool run --from 'maturin>=1.15,<2' maturin build --locked` to build
a wheel without installing it. No package is published by these commands.

## Python preview distributions

The Python version is in `pyproject.toml`; Rust crates keep their own versions.
The `Python distributions` workflow builds an optimized, stripped CPython
3.12+ stable-ABI wheel for Linux x86_64 (glibc 2.34+) and an sdist. It audits the
wheel's library requirements and installs both artifacts independently on
Python 3.12 and 3.14, including the RoomKit extra and the runnable example.
The workflow has read-only repository permissions and does not publish.

After the normal gates pass on the intended commit, run it from that branch or
tag using `gh workflow run python-distributions.yml --ref main`. Wait for the
entire run to succeed, then download its `python-distributions` artifact with
`gh run download RUN_ID --name python-distributions --dir dist`. Check
`SHA256SUMS` from that directory. Create the release tag on the run's exact
commit, matching the Python version, and attach those distributions and checksums.
Use a prerelease for alpha versions. Publishing on PyPI is a separate step.

To repeat the artifact checks locally with CPython 3.12+, Rust and uv:

```sh
python3 scripts/check-python.py --wheel dist/*.whl --sdist dist/*.tar.gz
```

The check builds from the source distribution only; wheel installation does not
compile Rust. Internal planning and integration assets are excluded from both
archives. Keep public package metadata, examples and release notes in English.

`tests/fixtures/contracts.json` supplies common reference scenarios for all 16
tools, including optional archive paging and explicit release. The Rust integration test checks expected behavior and file bytes.
`make python-check` also generates fresh Rust results and compares each installed
Python package against their complete JSON envelopes and final file bytes. Only
temporary root paths are normalized; error categories are mapped across languages.
Process observations wait for explicit state or byte counts. These maintainer
checks need Rust; installing and using the prebuilt wheel still does not.
The shared cases complement the larger per-language suites for concurrency,
large files, cancellation during I/O and cleanup.

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
