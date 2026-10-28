#!/usr/bin/env python3
"""Build a wheel, inspect its contents and test an isolated installation."""

from __future__ import annotations

import os
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from email.parser import BytesParser
from pathlib import Path


def run(*arguments: str, cwd: Path) -> None:
    subprocess.run(arguments, cwd=cwd, check=True)


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="roomkit-tools-python-") as temporary:
        work = Path(temporary)
        wheels = work / "wheels"
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
            dependencies = BytesParser().parsebytes(metadata).get_all("Requires-Dist", [])
            assert all(
                dependency.partition(";")[2].strip().replace("'", '"') == 'extra == "roomkit"'
                for dependency in dependencies
            ), "Core installation must have no Python dependencies"
        environment = work / "venv"
        run("uv", "venv", "--python", sys.executable, str(environment), cwd=work)
        python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
        run("uv", "pip", "install", "--python", str(python), "--no-deps", str(wheel), cwd=work)
        run(
            str(python),
            "-I",
            "-m",
            "unittest",
            "discover",
            "-s",
            str(root / "tests/python"),
            "-p",
            "test_read.py",
            "-v",
            cwd=work,
        )
        # An sdist must independently rebuild the mixed package, including path
        # dependencies, without bringing local planning or integration assets.
        sources = work / "sources"
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
        run(
            "uv",
            "pip",
            "install",
            "--python",
            str(python),
            "--reinstall",
            "--no-deps",
            str(source),
            cwd=work,
        )
        run(
            str(python),
            "-I",
            "-m",
            "unittest",
            "discover",
            "-s",
            str(root / "tests/python"),
            "-p",
            "test_read.py",
            "-v",
            cwd=work,
        )
        # Exercise the declared optional integration against its minimum release.
        run("uv", "pip", "install", "--python", str(python), "roomkit==0.110.0", cwd=work)
        run(
            str(python),
            "-I",
            "-m",
            "unittest",
            "discover",
            "-s",
            str(root / "tests/python"),
            "-p",
            "test_roomkit.py",
            "-v",
            cwd=work,
        )
        run(str(python), "-I", str(root / "examples/roomkit_read.py"), cwd=work)


if __name__ == "__main__":
    main()
