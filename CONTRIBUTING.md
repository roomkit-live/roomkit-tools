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

`make hooks` enables the optional commit-message and pre-push hooks (see
[One public API](#one-public-api) for the latter). Python development also
requires CPython 3.12+ and [uv](https://docs.astral.sh/uv/). Build/install locally
with `uv venv` and `uv pip install '.[roomkit]'`. See
[installation](guide/installation.md) for supported platforms and provider extras.

## Validation gates

| Command | Scope |
| --- | --- |
| `make fmt` | Format the standalone Rust workspace |
| `make all` | Formatting, Clippy, tests, rustdoc, dependency policy, core dependency allow list, executable Rust examples and the public API parity gate |
| `make api-test api-parity` | The parity gate's own tests, then the gate: the Rust public API and its defaults against the Python snapshots (see [One public API](#one-public-api)) |
| `make api-snapshot` | Rewrite the Python API snapshots from freshly built wheels |
| `make api-adapter-surface` | Rewrite the RoomKit adapter's committed Rust surface; requires access to its pinned engine source |
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
| `crates/roomkit-tools-web/tests/fixtures/web.json` | HTTP fetching through local servers |
| `tests/fixtures/search.json` | Search backends through local JSON/SSE fixtures |
| `tests/fixtures/outline.json` | Source symbols, ranges, revisions, errors and bounds |
| `crates/roomkit-tools-docs/tests/fixtures/documentation.json` | Documentation snapshots, search, provenance, pagination and complete Rust/Python envelopes |
| `crates/roomkit-tools-plans/tests/fixtures/plans.json` | Plan CRUD envelopes, stable step IDs, revisions and list/delete state |
| `tests/fixtures/image.json` | Complete image payloads, metadata and refusals |
| `tests/fixtures/roomkit_owner.json` | RoomKit owner keys, identical in the Rust adapter and in Python |

A fixture that a crate's own tests read lives in that crate's `tests/`
directory, since a published crate ships only its own directory; the Python
tests and the examples read it there.

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

## One public API

The Rust crates and the Python packages have one public API: for every
feature both have, the same names, parameters, defaults and behaviour.
Python is the reference.

- Before adding or changing a public item in Rust, read Python's in the
  snapshot (`tests/python/public_api_signatures.json`, the tool contracts in
  `public_api_contracts.json`, and the OCR package's in
  `backends/rusto/python/tests/`) and mirror it.
- Only what the language forces may differ: Rust's casing of an acronym,
  `&self`, a struct of named fields for keyword arguments, a `Result` for a
  raised exception, an `async fn` for an `async def`, and the like. Each such
  mapping is written in `api/mapping.toml`, as a rule or an approved
  `[[map]]` entry; `scripts/api-parity.py`'s docstring says what each rule
  does.
- Any other difference needs a maintainer's decision before it is written,
  whatever the reason. The decision either changes the code (Python first
  when Python changes, then Rust) or records the difference as approved in
  `api/mapping.toml`: a rule, a `[[map]]` or `[[exception]]` entry, or a
  `[[missing]]` entry for a feature only one side has. A difference is never
  shipped undiscussed: until the maintainer decides, it is a red gate. It
  may be recorded meanwhile as a `[[pending]]` entry, which pairs names and
  documents the question, but the gate fails while one is left.
- Every rule and every `[[map]]`, `[[exception]]` and `[[missing]]` entry
  names its source in `decision`: the roomkit-rs precedent it follows (a row
  of the names table in its `docs/architecture.md`, or a rule of its
  `api/mapping.toml`), or the maintainer's decision. The gate refuses one
  without. A rule or an entry with neither reads
  `decision = "TODO: maintainer"`, an entry then carrying no `approved`
  date, and the gate fails until the maintainer decides it.
- `make api-parity` enforces this: it reads the crates' public surface from
  rustdoc and fails on any difference no entry of `api/mapping.toml` covers,
  and on an entry that no longer covers anything. Defaults are compared by
  value: rustdoc shows none, so `examples/tests/api_defaults.rs` dumps the
  defaults of each struct a call's keywords stand for, and a parameter both
  sides default has the same value on both. Python's `None` for a Rust field
  that is no `Option` is the Rust default (`[rules] deferred_default`); any
  other difference, an `Option` Rust resolves at execution included, is a
  finding. A new such struct, or field, goes into that test: the gate fails
  on one the dump lacks, and a field it does not list fails to compile.
  `make api-parity`, CI and the pre-push hook fail on any `[[pending]]`
  entry and any `TODO: maintainer` decision. The gate and its tests need
  only `python3` (3.11 or newer) and its standard library.
- A public item hidden with `#[doc(hidden)]` is on no rustdoc page. The
  gate lists them from the JSON rustdoc writes with
  `--document-hidden-items`, unstable options the pinned toolchain takes
  for that build only (`RUSTC_BOOTSTRAP=1`, `make api-hidden`), and each
  needs a `[[hidden]]` entry naming its decision. The RoomKit adapter's
  hidden items are committed in `api/roomkit-adapter-hidden.json`, written
  and checked with its surface. The internal `roomkit-tools-http` crate is
  out of scope and must show no public item.
- `make hooks` enables a pre-push hook that runs `make api-test api-parity`
  when the pushed commits change what the gate reads: `crates/`,
  `adapters/`, `python/roomkit_tools/`, `api/`, `scripts/`, the defaults
  test, the Python snapshots, the root `Cargo.toml` and `Cargo.lock`,
  `rust-toolchain.toml` and the `Makefile`. It checks the checked-out tree,
  so it refuses to push another commit (an annotated tag is checked by its
  commit), or with uncommitted or untracked files there. It builds no
  wheel: CI checks the snapshots. It takes about a second on a built target
  directory and under a minute on an empty one on a large machine;
  `git push --no-verify` skips it.
- A change to the Python public API rewrites the snapshot with
  `make api-snapshot` in the same commit; the Python tests fail while the
  snapshot differs from the code. A change to the RoomKit adapter's public API
  rewrites `api/roomkit-adapter-surface.json` with
  `make api-adapter-surface`, which also rewrites the adapter's hidden items
  in `api/roomkit-adapter-hidden.json`. CI cannot check those files: the
  adapter's engine source is not public. `make roomkit-check` checks them,
  and so does the pre-push hook when the pushed commits change `adapters/`,
  either file, `scripts/rs-surface.py` or `rust-toolchain.toml`: it runs
  `make api-adapter-check`, and refuses the push when the check fails or
  cannot run without the pinned roomkit-rs source. The check ignores the
  rustdoc version the files name.

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
Keep local files out of version control with your checkout's `.git/info/exclude`
and do not force-add ignored material. The distribution checkers accept only
tracked files in source archives and expected paths in wheels, so an untracked
local file fails the check instead of being published.

See [release preparation](guide/releasing.md) for artifact validation and the
planned TestPyPI/PyPI flow. The existing distribution workflow only builds and
tests; it does not publish. Each wheel ships the license notices of every
native dependency in its `licenses/` directory. After a dependency change,
regenerate them with `python scripts/native-notices.py write roomkit-tools`
(and `roomkit-tools-ocr-rusto` for the OCR package); the checkers verify that
the shipped notices cover every locked dependency.

## Commits

Use `type(scope): summary`, at most 72 characters, for example
`feat(fs): add paginated text reading`. The hook and CI share the same checker.
Record user-visible changes under `Unreleased` in [CHANGELOG.md](CHANGELOG.md).
