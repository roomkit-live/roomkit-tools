#!/usr/bin/env python3
"""Build a wheel, inspect its contents and test an isolated installation."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
import tomllib
import zipfile
from email.parser import BytesParser
from pathlib import Path

import artifact_files

NOTICES = "roomkit_tools/licenses/native_dependencies.txt"
MATURIN = ("uv", "tool", "run", "--from", "maturin>=1.15,<2", "maturin")

# Rust tests that write the reference results the Python tests compare with:
# the environment variable naming the result file, its name, the cargo test.
REFERENCES = [
    (
        "ROOMKIT_DOCS_RESULTS",
        "rust-docs.json",
        ("-p", "roomkit-tools-docs", "--features", "files", "--test", "contracts"),
    ),
    (
        "ROOMKIT_PLAN_RESULTS",
        "rust-plans.json",
        ("-p", "roomkit-tools-plans", "--test", "contracts"),
    ),
    (
        "ROOMKIT_CONTRACT_RESULTS",
        "rust-contracts.json",
        ("-p", "roomkit-tools-examples", "--test", "contracts"),
    ),
    (
        "ROOMKIT_WEB_RESULTS",
        "rust-web.json",
        ("-p", "roomkit-tools-examples", "--test", "web_contracts"),
    ),
    (
        "ROOMKIT_SEARCH_RESULTS",
        "rust-search.json",
        ("-p", "roomkit-tools-examples", "--test", "search_contracts"),
    ),
    (
        "ROOMKIT_OUTLINE_RESULTS",
        "rust-outline.json",
        ("-p", "roomkit-tools-examples", "--test", "outline_contracts"),
    ),
    (
        "ROOMKIT_IMAGE_RESULTS",
        "rust-image.json",
        ("-p", "roomkit-tools-examples", "--test", "image_contracts"),
    ),
]

# Test files of the installation without RoomKit, in order, with the reference
# each one compares with.
CORE_TESTS = [
    ("test_docs.py", "ROOMKIT_DOCS_RESULTS"),
    ("test_read.py", None),
    ("test_ocr.py", None),
    ("test_agents.py", None),
    ("test_plans.py", "ROOMKIT_PLAN_RESULTS"),
    ("test_shutdown.py", None),
    ("test_discovery.py", None),
    ("test_mutations.py", None),
    ("test_diff_patch.py", None),
    ("test_shell.py", None),
    ("test_process.py", None),
    ("test_job_ends.py", None),
    ("test_output.py", None),
    ("test_web.py", "ROOMKIT_WEB_RESULTS"),
    ("test_search.py", "ROOMKIT_SEARCH_RESULTS"),
    ("test_outline.py", "ROOMKIT_OUTLINE_RESULTS"),
    ("test_image.py", "ROOMKIT_IMAGE_RESULTS"),
    ("test_contracts.py", "ROOMKIT_CONTRACT_RESULTS"),
    # The public API against its snapshot, the reference of `make api-parity`.
    ("test_public_api.py", None),
    ("test_definitions.py", None),
]
CORE_EXAMPLES = [
    ("discover_files.py",),
    ("edit_file.py",),
    ("patch_files.py",),
    ("run_shell.py",),
    ("run_process.py",),
]
ROOMKIT_EXAMPLES = [
    ("roomkit_docs.py",),
    ("roomkit_plans.py",),
    ("roomkit_read.py",),
    ("roomkit_code.py",),
    ("roomkit_outline.py",),
    ("roomkit_image.py",),
    ("roomkit_background.py",),
    ("roomkit_delegate.py",),
    ("roomkit_agents.py",),
    ("roomkit_agents.py", "--dynamic"),
    ("roomkit_agents.py", "--dynamic", "--parallel"),
    ("roomkit_output.py",),
    ("roomkit_web.py",),
    ("roomkit_search.py",),
    ("roomkit_search.py", "--service", "fixture-openai"),
]
# The minimum supported RoomKit release; vLLM's SDK is used only with a mock
# HTTP transport in tests.
ROOMKIT_REQUIREMENT = "roomkit[vllm]==0.110.0"


def run(*arguments: str, cwd: Path) -> None:
    subprocess.run(arguments, cwd=cwd, check=True)


def check_notices(wheel: zipfile.ZipFile, root: Path, work: Path) -> None:
    """The wheel's notices cover every locked crate linked into the extension."""
    assert NOTICES in wheel.namelist(), "the wheel has no native dependency notices"
    notices = work / "native_dependencies.txt"
    notices.write_bytes(wheel.read(NOTICES))
    run(
        sys.executable,
        str(root / "scripts/native-notices.py"),
        "check",
        "roomkit-tools",
        str(notices),
        cwd=root,
    )


def generate_references(root: Path, work: Path) -> dict[str, Path]:
    """Write each Rust reference result; return its path by environment variable."""
    references = {}
    for variable, name, test in REFERENCES:
        path = work / name
        subprocess.run(
            ["cargo", "test", "--locked", *test],
            cwd=root,
            env={**os.environ, variable: str(path)},
            check=True,
        )
        references[variable] = path
    return references


def build_wheel(root: Path, work: Path) -> Path:
    wheels = work / "wheels"
    run(
        *MATURIN,
        "build",
        "--locked",
        "--interpreter",
        sys.executable,
        "--out",
        str(wheels),
        cwd=root,
    )
    (wheel,) = wheels.glob("*.whl")
    return wheel


def build_sdist(root: Path, work: Path) -> Path:
    # An sdist must independently rebuild the mixed package, including path
    # dependencies, from tracked files only.
    sources = work / "sources"
    run(*MATURIN, "sdist", "--out", str(sources), cwd=root)
    (source,) = sources.glob("*.tar.gz")
    return source


def check_metadata(metadata: bytes, names: list[str], root: Path, version: str) -> None:
    """The wheel declares the version, description, licenses and links of the project."""
    message = BytesParser().parsebytes(metadata)
    assert message["Version"] == version, message["Version"]
    assert message["Description-Content-Type"].split(";", 1)[0] == "text/markdown"
    assert (
        message.get_payload(decode=True).decode("utf-8").strip()
        == (root / "README.md").read_text(encoding="utf-8").strip()
    )
    assert message["License-Expression"] == "MIT"
    license_files = message.get_all("License-File", [])
    assert "LICENSE" in license_files
    assert f"python/{NOTICES}" in license_files, license_files
    assert all(
        any(name.endswith(f".dist-info/licenses/{license_file}") for name in names)
        for license_file in license_files
    ), "Declared licenses must be included in the wheel"
    project_urls = tomllib.loads((root / "pyproject.toml").read_text())["project"]["urls"]
    assert set(message.get_all("Project-URL", [])) == {
        f"{label}, {url}" for label, url in project_urls.items()
    }
    dependencies = message.get_all("Requires-Dist", [])
    assert all(
        dependency.partition(";")[2].strip().replace("'", '"') == 'extra == "roomkit"'
        for dependency in dependencies
    ), "Core installation must have no Python dependencies"


def check_wheel(wheel: Path, root: Path, work: Path, version: str) -> None:
    """The wheel holds the extension, typing marker, notices and metadata, nothing else."""
    with zipfile.ZipFile(wheel) as archive:
        names = archive.namelist()
        assert any(name.endswith((".so", ".pyd")) for name in names), names
        assert "roomkit_tools/py.typed" in names, names
        check_notices(archive, root, work)
        metadata = archive.read(next(n for n in names if n.endswith("/METADATA")))
        check_metadata(metadata, names, root, version)
    # Untracked or ignored local files must not reach the wheel.
    artifact_files.check_wheel(wheel, root, "roomkit_tools")


def run_tests(
    python: Path, root: Path, work: Path, pattern: str, reference: dict[str, str]
) -> None:
    subprocess.run(
        [
            str(python),
            "-X",
            "faulthandler",
            "-I",
            "-m",
            "unittest",
            "discover",
            "-s",
            str(root / "tests/python"),
            "-p",
            pattern,
            "-v",
        ],
        cwd=work,
        env={**os.environ, **reference},
        check=True,
    )


def run_examples(python: Path, root: Path, work: Path, examples: list[tuple[str, ...]]) -> None:
    for script, *arguments in examples:
        run(str(python), "-I", str(root / "examples" / script), *arguments, cwd=work)


def install_and_test(
    label: str, package: Path, root: Path, work: Path, references: dict[str, Path]
) -> None:
    """Install one artifact alone, test it, then add RoomKit and test the adapters."""
    environment = work / label
    run("uv", "venv", "--python", sys.executable, str(environment), cwd=work)
    python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    run("uv", "pip", "install", "--python", str(python), "--no-deps", str(package), cwd=work)
    for pattern, variable in CORE_TESTS:
        reference = {variable: str(references[variable])} if variable else {}
        run_tests(python, root, work, pattern, reference)
    run_examples(python, root, work, CORE_EXAMPLES)
    # Exercise the declared integration against its minimum release.
    run(
        "uv",
        "pip",
        "install",
        "--python",
        str(python),
        f"{package}[roomkit]",
        ROOMKIT_REQUIREMENT,
        cwd=work,
    )
    run_tests(python, root, work, "test_roomkit*.py", {})
    run_examples(python, root, work, ROOMKIT_EXAMPLES)


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    # Extracted sdists live outside the checkout's rustup override.
    os.environ.setdefault(
        "RUSTUP_TOOLCHAIN",
        tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"],
    )
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wheel", type=Path, help="Validate this prebuilt wheel")
    parser.add_argument("--sdist", type=Path, help="Validate this source distribution")
    options = parser.parse_args()
    if bool(options.wheel) != bool(options.sdist):
        parser.error("--wheel and --sdist must be supplied together")
    version = tomllib.loads((root / "pyproject.toml").read_text())["project"]["version"]
    with tempfile.TemporaryDirectory(prefix="roomkit-tools-python-") as temporary:
        work = Path(temporary)
        # Build reference results from the Rust API before comparing installed
        # Python packages. This is a maintainer check, not an installation step.
        references = generate_references(root, work)
        wheel = options.wheel.resolve() if options.wheel else build_wheel(root, work)
        check_wheel(wheel, root, work, version)
        source = options.sdist.resolve() if options.sdist else build_sdist(root, work)
        artifact_files.check_sdist(source, root)
        # Validate both artifacts independently, including their RoomKit extra.
        for label, package in [("wheel", wheel), ("sdist", source)]:
            install_and_test(label, package, root, work, references)


if __name__ == "__main__":
    main()
