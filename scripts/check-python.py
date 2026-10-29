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
            run_tests(python, root, work, "test_discovery.py")
            run_tests(python, root, work, "test_mutations.py")
            run_tests(python, root, work, "test_diff_patch.py")
            run_tests(python, root, work, "test_shell.py")
            run_tests(python, root, work, "test_process.py")
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


def run_tests(python: Path, root: Path, work: Path, pattern: str) -> None:
    run(
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
        cwd=work,
    )


if __name__ == "__main__":
    main()
