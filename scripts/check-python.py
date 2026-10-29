#!/usr/bin/env python3
"""Build a wheel, inspect its contents and test an isolated installation."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from email.parser import BytesParser
from pathlib import Path


def run(*arguments: str, cwd: Path) -> None:
    subprocess.run(arguments, cwd=cwd, check=True)


def main() -> None:
    root = Path(__file__).resolve().parents[1]
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
        reference = work / "rust-contracts.json"
        subprocess.run(
            ["cargo", "test", "--locked", "-p", "roomkit-tools-examples", "--test", "contracts"],
            cwd=root,
            env={**os.environ, "ROOMKIT_CONTRACT_RESULTS": str(reference)},
            check=True,
        )
        web_reference = work / "rust-web.json"
        subprocess.run(
            [
                "cargo",
                "test",
                "--locked",
                "-p",
                "roomkit-tools-examples",
                "--test",
                "web_contracts",
            ],
            cwd=root,
            env={**os.environ, "ROOMKIT_WEB_RESULTS": str(web_reference)},
            check=True,
        )
        search_reference = work / "rust-search.json"
        subprocess.run(
            [
                "cargo",
                "test",
                "--locked",
                "-p",
                "roomkit-tools-examples",
                "--test",
                "search_contracts",
            ],
            cwd=root,
            env={**os.environ, "ROOMKIT_SEARCH_RESULTS": str(search_reference)},
            check=True,
        )
        outline_reference = work / "rust-outline.json"
        subprocess.run(
            [
                "cargo",
                "test",
                "--locked",
                "-p",
                "roomkit-tools-examples",
                "--test",
                "outline_contracts",
            ],
            cwd=root,
            env={**os.environ, "ROOMKIT_OUTLINE_RESULTS": str(outline_reference)},
            check=True,
        )
        wheels = work / "wheels"
        if options.wheel:
            wheel = options.wheel.resolve()
        else:
            run(
                "uv",
                "tool",
                "run",
                "--from",
                "maturin>=1.15,<2",
                "maturin",
                "build",
                "--locked",
                "--interpreter",
                sys.executable,
                "--out",
                str(wheels),
                cwd=root,
            )
            (wheel,) = wheels.glob("*.whl")
        with zipfile.ZipFile(wheel) as archive:
            names = archive.namelist()
            assert any(name.endswith((".so", ".pyd")) for name in names), names
            assert "roomkit_tools/py.typed" in names, names
            assert "roomkit_tools/licenses/web_transport.txt" in names, names
            assert "roomkit_tools/licenses/code_parsing.txt" in names, names
            assert not any(
                name.startswith(("docs/", "integrations/"))
                or "bakery" in name
                or name.endswith("AGENTS.md")
                for name in names
            ), names
            metadata = archive.read(next(n for n in names if n.endswith("/METADATA")))
            metadata = BytesParser().parsebytes(metadata)
            assert metadata["Version"] == version, metadata["Version"]
            dependencies = metadata.get_all("Requires-Dist", [])
            assert all(
                dependency.partition(";")[2].strip().replace("'", '"') == 'extra == "roomkit"'
                for dependency in dependencies
            ), "Core installation must have no Python dependencies"
        # An sdist must independently rebuild the mixed package, including path
        # dependencies, without bringing local planning or integration assets.
        sources = work / "sources"
        if options.sdist:
            source = options.sdist.resolve()
        else:
            run(
                "uv",
                "tool",
                "run",
                "--from",
                "maturin>=1.15,<2",
                "maturin",
                "sdist",
                "--out",
                str(sources),
                cwd=root,
            )
            (source,) = sources.glob("*.tar.gz")
        with tarfile.open(source) as archive:
            names = archive.getnames()
            assert not any(
                "/docs/" in name
                or "/integrations/" in name
                or "bakery" in name
                or name.endswith("/AGENTS.md")
                for name in names
            ), names
        # Validate both artifacts independently, including their RoomKit extra.
        for label, package in [("wheel", wheel), ("sdist", source)]:
            environment = work / label
            run("uv", "venv", "--python", sys.executable, str(environment), cwd=work)
            python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
            run(
                "uv", "pip", "install", "--python", str(python), "--no-deps", str(package), cwd=work
            )
            run_tests(python, root, work, "test_read.py")
            run_tests(python, root, work, "test_shutdown.py")
            run_tests(python, root, work, "test_discovery.py")
            run_tests(python, root, work, "test_mutations.py")
            run_tests(python, root, work, "test_diff_patch.py")
            run_tests(python, root, work, "test_shell.py")
            run_tests(python, root, work, "test_process.py")
            run_tests(python, root, work, "test_output.py")
            run_tests(python, root, work, "test_web.py", web_reference=web_reference)
            run_tests(python, root, work, "test_search.py", search_reference=search_reference)
            run_tests(python, root, work, "test_outline.py", outline_reference=outline_reference)
            run_tests(python, root, work, "test_contracts.py", reference=reference)
            run(str(python), "-I", str(root / "examples/discover_files.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/edit_file.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/patch_files.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/run_shell.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/run_process.py"), cwd=work)
            # Exercise the declared integration against its minimum release.
            run(
                "uv",
                "pip",
                "install",
                "--python",
                str(python),
                f"{package}[roomkit]",
                "roomkit==0.110.0",
                cwd=work,
            )
            run_tests(python, root, work, "test_roomkit*.py")
            run(str(python), "-I", str(root / "examples/roomkit_read.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/roomkit_code.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/roomkit_outline.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/roomkit_background.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/roomkit_output.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/roomkit_web.py"), cwd=work)
            run(str(python), "-I", str(root / "examples/roomkit_search.py"), cwd=work)
            run(
                str(python),
                "-I",
                str(root / "examples/roomkit_search.py"),
                "--service",
                "fixture-openai",
                cwd=work,
            )


def run_tests(
    python: Path,
    root: Path,
    work: Path,
    pattern: str,
    *,
    reference: Path | None = None,
    web_reference: Path | None = None,
    search_reference: Path | None = None,
    outline_reference: Path | None = None,
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
        env={
            **os.environ,
            **({"ROOMKIT_CONTRACT_RESULTS": str(reference)} if reference else {}),
            **({"ROOMKIT_WEB_RESULTS": str(web_reference)} if web_reference else {}),
            **({"ROOMKIT_SEARCH_RESULTS": str(search_reference)} if search_reference else {}),
            **({"ROOMKIT_OUTLINE_RESULTS": str(outline_reference)} if outline_reference else {}),
        },
        check=True,
    )


if __name__ == "__main__":
    main()
