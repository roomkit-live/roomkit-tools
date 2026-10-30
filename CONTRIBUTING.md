# Contributing

Start with the [project overview](README.md), [public guides](guide/README.md)
and [Rust API boundaries](guide/rust.md). Keep public behavior, examples and
resource limits consistent with the implementation.

## Development environment

Rustup selects the toolchain pinned in `rust-toolchain.toml`; the MSRV is 1.94.
Install cargo-deny for dependency checks:

```sh
cargo install cargo-deny --locked
make hooks
```

`make hooks` enables the optional commit-message hook. Python development also
requires CPython 3.12+ and [uv](https://docs.astral.sh/uv/). Build/install locally
with `uv venv` and `uv pip install '.[roomkit]'`. See
[installation](guide/installation.md) for supported platforms and provider extras.

## Validation gates

| Command | Scope |
| --- | --- |
| `make fmt` | Format the standalone Rust workspace |
| `make all` | Formatting, Clippy, tests, rustdoc, dependency policy, core dependency allow list and executable Rust examples |
| `make msrv-toolchain msrv` | Install/check the minimum Rust toolchain |
| `make python-check` | Binding lint/docs/dependencies, Python lint, independent wheel/source installs, standalone and RoomKit tests and examples |
| `make ocr-python-check` | Optional engine wheel/source installs and RoomKit selection; set `ROOMKIT_OCR_MODELS` for real-model parity |
| `make ocr-python-msrv` | Check the optional OCR extension on the MSRV |
| `make python-msrv` | Check native Python bindings on the MSRV |
| `make roomkit-check roomkit-msrv` | Optional Rust RoomKit adapter; requires access to its pinned engine source |

Run relevant checks during development, then `make all`. For Python/package
changes, also run `make python-check`. Run MSRV checks after adding dependencies
or using newer standard-library APIs. The optional Rust adapter is a separate
workspace; its gates are deliberately independent of standalone CI.

The Python artifact checks install the package without Python dependencies first,
then add the minimum supported RoomKit release for adapter tests. The vLLM extra
is used with a mock HTTP transport for image serialization tests, not a live
server. CI exercises standard CPython 3.12 and 3.14.

With populated Cargo caches and an advisory database, an offline check can use
`CARGO_NET_OFFLINE=true make DENY_FLAGS=--offline all`. This checks cached
advisories only; normal CI refreshes them. Required checkers must not be silently
skipped.

## Contracts and tests

Test observable behavior, especially boundaries, side effects, cancellation and
cleanup. Filesystem scenarios use temporary directories; default checks require
no external service or credentials. Live-model examples are additional opt-in
validation and do not replace deterministic tests.

| Shared fixture | Coverage |
| --- | --- |
| `tests/fixtures/contracts.json` | Filesystem, commands, persistent processes and output archives |
| `tests/fixtures/web.json` | HTTP fetching through local servers |
| `tests/fixtures/search.json` | Search backends through local JSON/SSE fixtures |
| `tests/fixtures/outline.json` | Source symbols, ranges, revisions, errors and bounds |
| `tests/fixtures/image.json` | Complete image payloads, metadata and refusals |

`scripts/check-python.py` generates fresh Rust results, then compares installed
Python packages with complete envelopes and file contents. It normalizes only
temporary paths and maps typed error categories. The broader suites cover
concurrency, in-flight cancellation, cleanup and actual RoomKit tool delivery.
Process tests wait for explicit state or byte counts rather than arbitrary delays.

Each `examples/*.rs` file must be declared in `examples/Cargo.toml`, check what it
shows, and exit unsuccessfully if those observations change. The
[example index](examples/README.md) distinguishes simple integration examples
from larger validation scenarios.

When changing search dependencies, run
`cargo test -p roomkit-tools-search --no-default-features` and check each affected
provider feature separately. No provider key belongs in a fixture or test command.

## Implementation boundaries

- Keep `roomkit-tools-core` free of I/O, runtimes, Python and RoomKit dependencies.
- Derive schemas from the Rust types used by implementations.
- Put dependency versions in `workspace.dependencies`; consuming crates enable
  only the features they need. Update the core allow list for any core change.
- Keep parser/grammar dependencies in the code crate and codecs in the image crate.
- Keep host configuration, cancellation, ownership and limits explicit.
- Document public items; return typed failures instead of panicking on caller input.
- Prefer `#[expect(lint, reason = "...")]` for a justified lint exception.

## Documentation and distribution

Public guides live in `guide/`; runnable examples are indexed in `examples/README.md`.
The root README is also the Python package description, so its documentation
links use absolute URLs that work on an index page. Keep it concise; put detailed
contracts and budgets in the guides. Verify changed snippets and local link targets.

Code, comments, examples and public documentation use English and generic data.
Keep local planning and private integration assets ignored and excluded from
archives. Do not force-add ignored material. The distribution checker rejects
internal documentation/integration assets in the wheel and source archive.

See [release preparation](guide/releasing.md) for artifact validation and the
planned TestPyPI/PyPI flow. The existing distribution workflow only builds and
tests; it does not publish. Native dependency license notices belong in the
package's `licenses/` directory and must remain in the wheel.

## Commits

Use `type(scope): summary`, at most 72 characters, for example
`feat(fs): add paginated text reading`. The hook and CI share the same checker.
Record user-visible changes under `Unreleased` in [CHANGELOG.md](CHANGELOG.md).
