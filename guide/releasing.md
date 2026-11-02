# Release preparation

This page describes the current artifact workflow and the proposed first PyPI
publication. This repository currently contains a build/validation workflow,
not a PyPI publishing job. Publisher configuration on PyPI must be verified
before an upload. Building, validating and uploading are separate operations.

## Current distribution scope

The Python version lives in `pyproject.toml`. Rust crates keep separate
versions; the library crates are crates.io packages (see
[Rust crates](#rust-crates)). Continue the existing alpha sequence until the API
is ready for a stable release. Select the final version before building release
artifacts; release-specific changes belong in the changelog.

The existing [distribution workflow](../.github/workflows/python-distributions.yml)
builds optimized, stripped CPython 3.12+ stable-ABI wheels for Linux x86_64
(glibc 2.34+) and source distributions for the base package and optional OCR
backend. Both wheels are compiled inside a pinned manylinux 2.34 container and
audited against that baseline; the host runner is not the wheel build environment.
It checks package metadata, hashes the files, and tests wheel/source
installations independently on CPython 3.12 and 3.14. The
workflow has read-only repository permissions and does not upload to an index.
Other wheel platforms need their own build and validation work before release.

## Prepare and validate a candidate

1. Set the intended version in `pyproject.toml` and
   `backends/rusto/python/pyproject.toml`, including the backend dependency floor.
   Update the changelog and installation instructions to describe only artifacts that will be shipped.
2. Run `make all`, `make python-check`, and the applicable MSRV checks. Check
   public Markdown links, examples and metadata. Keep internal plans, fixtures
   and credentials out of version control and both distributions. After a
   dependency change, regenerate the native notices of each package with
   `python scripts/native-notices.py write roomkit-tools` (or
   `roomkit-tools-ocr-rusto`); the artifact checks refuse a wheel whose notices
   miss a locked dependency.
3. Commit the candidate and validate that exact commit through CI, or the
   equivalent recorded checks described below if hosted runners are unavailable.
   A job that never starts does not validate a release. Source access for linked public
   documentation must also be ready before advertising it on PyPI.
4. Dispatch the distribution workflow on the candidate ref:

   ```sh
   gh workflow run python-distributions.yml --ref main
   ```

   Confirm the run's `headSha` matches the candidate and wait for all jobs to pass.
   Download into a new, empty directory:

   ```sh
   gh run download RUN_ID --name python-distributions --dir release-candidate
   cd release-candidate
   sha256sum --check SHA256SUMS
   ```

5. Keep the tested files unchanged. Check their version, wheel compatibility tag,
   license notices and source contents. Tag the exact tested commit when the
   release is approved. Do not rebuild different bytes for publication.

If hosted runners are unavailable, reproduce the build in a compatible Linux
container and run the same Rust/binding gates and artifact checks locally. Audit
the wheel's manylinux compatibility and test both wheel and source installs on
CPython 3.12 and 3.14. Record the exact commit, container base digest, toolchain,
checksums, checks performed and hosted-CI limitation with the release. Do not
relabel a binary that fails compatibility auditing or report local checks as a
successful hosted CI run. This fallback does not publish to PyPI automatically.

To repeat package validation locally, from the repository root:

```sh
uv tool run --from 'twine>=6,<7' twine check --strict \
  release-candidate/*.whl release-candidate/*.tar.gz
python scripts/check-python.py \
  --wheel release-candidate/roomkit_tools-*.whl \
  --sdist release-candidate/roomkit_tools-*.tar.gz
python scripts/check-ocr-python.py \
  --base-wheel release-candidate/roomkit_tools-*.whl \
  --wheel release-candidate/roomkit_tools_ocr_rusto-*.whl \
  --sdist release-candidate/roomkit_tools_ocr_rusto-*.tar.gz
```

The validation scripts install the artifacts independently and compare their
behavior with fresh Rust reference results. They refuse a wheel or source
archive holding any file that git does not track, other than build metadata
and the native module, so run them from the candidate's checkout. They need Rust and do not upload.
Set `ROOMKIT_OCR_MODELS` to a pre-provisioned model directory to additionally
check real native recognition and RoomKit delivery; no models are downloaded.
`twine check` validates metadata and description rendering, not application behavior.

## Rust crates

The library crates under `crates/` are crates.io packages. The examples, the two
Python native modules and the RoomKit adapter are not: each sets
`publish = false` and says why. The adapter depends on roomkit-rs by Git, which
crates.io refuses. Internal dependencies give both `path` and `version`, so a
packaged crate depends on the published version of its siblings; change those
versions together with `workspace.package.version`. The unpublished packages
carry the same version: the examples inherit it, and the native modules and the
adapter, each a separate workspace, set it and their internal requirements in
their own manifests. A pre-release such as `0.1.0-alpha.16` matches only a
requirement naming a pre-release, so every internal requirement changes with
it. Update each workspace's `Cargo.lock` with `cargo update --workspace
--offline` (`--manifest-path` for the separate workspaces) and check that its
diff changes only the roomkit-tools packages.

From the candidate's checkout, package every published crate, build each
against the others in a local registry and stop before the upload, then list
the files one package ships:

```sh
cargo publish --workspace --dry-run --locked
cargo package --list --package roomkit-tools-fs
```

The dry run skips the crates with `publish = false`. A fixture a crate's own
tests read lives in that crate, so its tests also pass from the packaged files.
Publishing itself is a maintainer's step, from the tested commit.

## Proposed first PyPI publication

Use [PyPI Trusted Publishing](https://docs.pypi.org/trusted-publishers/using-a-publisher/)
with GitHub Actions rather than a long-lived upload token. A future dedicated
publishing job should consume the already-tested distribution artifact, depend
on successful validation, use a named GitHub environment, and request
`id-token: write` only in that publishing job.

Before implementing that job, choose the PyPI account or organization that owns
`roomkit-tools` and confirm the project name. For a new project, a
[pending trusted publisher](https://docs.pypi.org/trusted-publishers/creating-a-project-through-oidc/)
can create it on first upload; registration is not a reservation of the name.
Its repository, workflow filename and environment must match the publishing job.
Configure TestPyPI and PyPI separately.

Rehearse on [TestPyPI](https://packaging.python.org/en/latest/guides/using-testpypi/)
first. Install the standalone candidate with `--no-deps` from that index; install
RoomKit's dependencies from the normal index separately. This avoids relying on
TestPyPI's incomplete dependency catalogue. Validate the candidate's import and
an actual tool call, then publish those same tested files to PyPI when approved.

After publication, verify installation from PyPI in a fresh environment, check
the project page and links, and change the README's primary installation command
to the published version. An alpha requires an explicit version or `--pre` where
appropriate. Do not advertise `pip install roomkit-tools` as available before
publication succeeds. PyPI does not allow reusing uploaded distribution filenames;
fix a released artifact with a new version.

[Documentation index](README.md) · [Project overview](../README.md)
