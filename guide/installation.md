# Installation and compatibility

The package is in alpha. Python package versions and Rust crate versions are
managed separately; APIs may change between previews.

## Availability

| Source | Availability |
| --- | --- |
| Current checkout | Package version declared in `pyproject.toml` |
| GitHub preview `v0.1.0a15` | Base package and optional OCR backend: wheels and source distributions |
| PyPI | First publication is being prepared; no install-from-PyPI command is available yet |
| crates.io | No published crates; use a checkout or Git dependency |

Preview wheels and source distributions are attached to
[GitHub releases](https://github.com/roomkit-live/roomkit-tools/releases). This
guide does not assume that a future PyPI release has already happened.

## From source

Use CPython 3.12 or newer, a Rust toolchain satisfying the workspace MSRV (1.94),
and a native linker/build environment. The pinned toolchain in
[`rust-toolchain.toml`](../rust-toolchain.toml) is used when building in a checkout.
From its root:

```sh
python3 -m venv .venv
source .venv/bin/activate
python -m pip install .
```

To include the RoomKit Python adapter's dependency:

```sh
python -m pip install '.[roomkit]'
```

An alternative with [uv](https://docs.astral.sh/uv/):

```sh
uv venv --python 3.12
uv pip install '.[roomkit]'
.venv/bin/python examples/roomkit_read.py
```

Maturin builds the native extension. The standalone package has no Python
runtime dependencies. The `roomkit` extra requires `roomkit>=0.110,<0.112`.
Provider-specific extras, such as `roomkit[vllm]`, are installed separately.

## From an existing GitHub wheel

A prebuilt wheel does not require Rust. Pip can install it from its release URL:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install \
  "roomkit-tools[roomkit] @ https://github.com/roomkit-live/roomkit-tools/releases/download/v0.1.0a15/roomkit_tools-0.1.0a15-cp312-abi3-manylinux_2_34_x86_64.whl"
```

To check its checksum first, download the wheel with the release's `SHA256SUMS`
and install the local file:

```sh
gh release download v0.1.0a15 --repo roomkit-live/roomkit-tools \
  --pattern 'roomkit_tools-*.whl' --pattern 'SHA256SUMS' --dir dist
(cd dist && sha256sum --check --ignore-missing SHA256SUMS)
.venv/bin/python -m pip install \
  './dist/roomkit_tools-0.1.0a15-cp312-abi3-manylinux_2_34_x86_64.whl[roomkit]'
```

A source distribution is also attached to the release; installing it compiles
Rust and needs the build environment described above. Runnable examples live in
the repository, not the installed import package.

## Supported configurations

| Component | Validated configuration |
| --- | --- |
| Python interpreter | Standard CPython 3.12 and 3.14; stable ABI targeting 3.12+ |
| Published preview wheel | Linux x86_64, glibc 2.34+ |
| Rust minimum | 1.94 |
| RoomKit Python adapter | Dependency range `>=0.110,<0.112`; automated checks use 0.110.0 |
| Shell and persistent processes | Linux; Unix process-group implementation |
| macOS, Windows, ARM64, musl, PyPy, free-threaded Python | No validated distributions in this project yet |

The stable ABI allows compatible standard CPython versions; it is not a claim
that every version or platform has been tested. Source builds on other targets
are not a supported-platform promise. Process tools return an unsupported error
on non-Unix systems.

## Verify the installation

```sh
python -c 'from importlib.metadata import version; print(version("roomkit-tools"))'
python -c 'from roomkit_tools import ReadTool; print(ReadTool.definition()["name"])'
```

From a checkout, `python examples/discover_files.py` exercises native filesystem
operations. With the RoomKit extra, `python examples/roomkit_read.py` also checks
an actual RoomKit tool call using a scripted provider. Neither needs an API key.

[Documentation index](README.md) · [Project overview](../README.md)

## Optional OCR engine

The `roomkit-tools-ocr-rusto` backend is a separate package. Download its wheel
only when selecting local OCR:

```sh
gh release download v0.1.0a15 --repo roomkit-live/roomkit-tools \
  --pattern 'roomkit_tools_ocr_rusto-*.whl' --dir dist
python -m pip install \
  './dist/roomkit_tools-0.1.0a15-cp312-abi3-manylinux_2_34_x86_64.whl[roomkit]' \
  './dist/roomkit_tools_ocr_rusto-0.1.0a15-cp312-abi3-manylinux_2_34_x86_64.whl'
```

Use the same Python environment as above and compare both wheel hashes with
`SHA256SUMS`. Installing both local files lets the dependency resolve without
looking for the base package on PyPI. Source builds of either package require
Rust 1.94+. The base package does not include inference dependencies or model
weights. Provision models explicitly; see [OCR configuration](ocr.md).
