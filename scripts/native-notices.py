#!/usr/bin/env python3
"""Write or check the third-party notices bundled with a native Python package.

License texts come from the locked Cargo registry sources, including license
files nested in a package; no notice is fetched from the network. Run from any
directory after a dependency change:

    python scripts/native-notices.py write roomkit-tools
    python scripts/native-notices.py write roomkit-tools-ocr-rusto
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
NOTICE_PREFIXES = ("license", "copying", "copyright", "notice")


@dataclass(frozen=True)
class Distribution:
    manifest: str
    output: str
    title: str
    # For packages with no notice file, keyed by a "name-version" glob: files
    # of the package source that hold its license, or upstream copies kept in
    # the repository (paths relative to its root).
    crate_files: dict[str, tuple[str, ...]] = field(default_factory=dict)
    upstream_files: dict[str, tuple[str, ...]] = field(default_factory=dict)


OCR_UPSTREAM = "backends/rusto/python/licenses/upstream"
DISTRIBUTIONS = {
    "roomkit-tools": Distribution(
        manifest="bindings/python/Cargo.toml",
        output="python/roomkit_tools/licenses/native_dependencies.txt",
        title=(
            "Third-party notices for the roomkit-tools native extension.\n\n"
            "License texts from locked dependency sources.\n"
        ),
        # The license and copyright notices are kept in this file.
        crate_files={"r-efi-6.0.0": ("AUTHORS",)},
    ),
    "roomkit-tools-ocr-rusto": Distribution(
        manifest="backends/rusto/python/Cargo.toml",
        output="backends/rusto/python/licenses/native_dependencies.txt",
        title=(
            "Third-party notices for the optional RustO OCR distribution.\n\n"
            "License texts from locked dependency sources; model weights are not included.\n"
        ),
        upstream_files={
            "rten-*0.27.0": (
                f"{OCR_UPSTREAM}/rten-0.27.0-Apache.txt",
                f"{OCR_UPSTREAM}/rten-0.27.0-MIT.txt",
            ),
            "flatbuffers-24.12.23": (f"{OCR_UPSTREAM}/flatbuffers-24.12.23.txt",),
        },
    ),
}


def cargo_metadata(distribution: Distribution) -> dict[str, Any]:
    return json.loads(
        subprocess.check_output(
            [
                "cargo",
                "metadata",
                "--manifest-path",
                str(ROOT / distribution.manifest),
                "--format-version",
                "1",
                "--locked",
            ],
            text=True,
        )
    )


def key(package: dict[str, Any]) -> str:
    return f"{package['name']}-{package['version']}"


def normal_dependencies(metadata: dict[str, Any]) -> list[str]:
    """Locked non-path packages reachable from the root by normal dependencies."""
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    reached: set[str] = set()
    pending = [metadata["resolve"]["root"]]
    while pending:
        current = pending.pop()
        if current in reached:
            continue
        reached.add(current)
        pending.extend(
            dependency["pkg"]
            for dependency in nodes[current]["deps"]
            if any(kind["kind"] is None for kind in dependency["dep_kinds"])
        )
    return sorted(
        key(packages[package_id])
        for package_id in reached
        if packages[package_id]["source"] is not None
    )


def configured(files: dict[str, tuple[str, ...]], package: dict[str, Any]) -> tuple[str, ...]:
    for pattern, paths in files.items():
        if fnmatch.fnmatchcase(key(package), pattern):
            return paths
    return ()


def notice_files(distribution: Distribution, package: dict[str, Any]) -> list[tuple[str, Path]]:
    """Notice files of one package as (name shown in its heading, path).

    The package's own notices come first, then those of nested components.
    """
    source = Path(package["manifest_path"]).parent
    found = sorted(
        path
        for path in source.rglob("*")
        if path.is_file() and path.name.lower().startswith(NOTICE_PREFIXES)
    )
    own = [path for path in found if path.parent == source]
    if not own:
        own = [source / name for name in configured(distribution.crate_files, package)]
        own += [ROOT / name for name in configured(distribution.upstream_files, package)]
    if not own:
        raise RuntimeError(f"Inspect missing notices for {package['name']} {package['version']}")
    nested = [path for path in found if path.parent != source]
    return [
        (path.relative_to(source).as_posix() if path.is_relative_to(source) else path.name, path)
        for path in own + nested
    ]


def render(distribution: Distribution, metadata: dict[str, Any]) -> str:
    parts = [distribution.title]
    packages = sorted(metadata["packages"], key=lambda item: (item["name"], item["version"]))
    for package in packages:
        if package["source"] is None:
            continue
        for name, path in notice_files(distribution, package):
            parts.append(
                f"\n--- {key(package)}/{name} ({package['license']}) ---\n\n"
                + path.read_text(encoding="utf-8")
            )
    text = "".join(parts)
    return "\n".join(line.rstrip() for line in text.splitlines()) + "\n"


def uncovered(distribution: Distribution, notices: str) -> list[str]:
    """Locked packages of the shipped dependency graph that have no notice."""
    return [
        package
        for package in normal_dependencies(cargo_metadata(distribution))
        if f"\n--- {package}/" not in notices
    ]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("action", choices=["write", "check"])
    parser.add_argument("distribution", choices=sorted(DISTRIBUTIONS))
    parser.add_argument("notices", nargs="?", type=Path, help="Notices file to check")
    options = parser.parse_args()
    distribution = DISTRIBUTIONS[options.distribution]
    if options.action == "write":
        text = render(distribution, cargo_metadata(distribution))
        (ROOT / distribution.output).write_text(text, encoding="utf-8")
        return
    notices = options.notices or ROOT / distribution.output
    missing = uncovered(distribution, notices.read_text(encoding="utf-8"))
    if missing:
        sys.exit(f"{notices}: no notice for {', '.join(missing)}")
    print(f"{notices}: notices cover every locked normal dependency")


if __name__ == "__main__":
    main()
