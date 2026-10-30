# Documentation

These guides describe the current source tree. For a released artifact, use the
documentation at its matching Git tag. The [installation guide](installation.md)
distinguishes development builds from available previews.

## Getting started

1. [Install and check compatibility](installation.md).
2. [Use the Python API or select RoomKit tools](python.md).
3. [Run an example](../examples/README.md) without credentials, then optionally
   connect your own model.
4. [Integrate the Rust crates](rust.md) if your host is written in Rust.

## Tool reference

| Guide | Topics |
| --- | --- |
| [Filesystem](filesystem.md) | Reading, discovery, revisions, writes, exact edits and structured patches |
| [Source outlines](code.md) | Rust/Python symbols, signatures, ranges and parsing limits |
| [Processes and archives](processes.md) | Shell, persistent stdin/stdout, deadlines, logs and cleanup |
| [Web](web.md) | Fetching, origin policy, search providers, citations and budgets |
| [Images](images.md) | Supported formats, preview preparation and multimodal results |
| [OCR](ocr.md) | Local text recognition, separate engines, models and positioned blocks |
| [Documentation tools](documentation.md) | Configured catalogues, lexical search, source lines and immutable snapshots |
| [Plans](plans.md) | Selectable CRUD, scoped storage, revisions and declared progress |
| [Agent delegation](agents.md) | Static/dynamic profiles, messages, follow-ups, task state and delivery |

## Maintainers

- [Contributing](../CONTRIBUTING.md): development environment and validation gates.
- [Release preparation](releasing.md): distributions and the planned first PyPI release.
- [Changelog](../CHANGELOG.md): released and unreleased behavior.

[Project overview](../README.md)
