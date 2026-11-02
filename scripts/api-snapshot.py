#!/usr/bin/env python3
"""Rewrite the public Python API snapshots from freshly built wheels.

Builds the base package and the optional OCR package, installs both in an
isolated environment, and writes their snapshots with
tests/python/api_signatures.py: tests/python/ for roomkit_tools (with the
tool contracts), backends/rusto/python/tests/ for roomkit_tools_ocr_rusto.
The Python tests fail while a snapshot differs from the code, and
scripts/api-parity.py compares the Rust crates with these files.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

MATURIN = ("uv", "tool", "run", "--from", "maturin>=1.15,<2", "maturin")
SNAPSHOTS = (
    ("roomkit_tools", "tests/python", ("--contracts",)),
    ("roomkit_tools_ocr_rusto", "backends/rusto/python/tests", ()),
)


def run(*arguments: str, cwd: Path, env: dict[str, str] | None = None) -> None:
    subprocess.run(arguments, cwd=cwd, env=env, check=True)


def build(directory: Path, out: Path, target: str | None = None) -> Path:
    """Build a wheel of the project in ``directory``, its Cargo artifacts in
    the subdirectory ``target`` of a shared CARGO_TARGET_DIR: both native
    libraries are named _native, and one shared output would package the
    other's."""
    env = dict(os.environ)
    if target and env.get("CARGO_TARGET_DIR"):
        env["CARGO_TARGET_DIR"] = str(Path(env["CARGO_TARGET_DIR"]) / target)
    run(
        *MATURIN,
        "build",
        "--locked",
        "--interpreter",
        sys.executable,
        "--out",
        str(out),
        cwd=directory,
        env=env,
    )
    (wheel,) = out.glob("*.whl")
    return wheel


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--wheel", type=Path, help="a prebuilt base package wheel")
    parser.add_argument("--ocr-wheel", type=Path, help="a prebuilt OCR package wheel")
    options = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="roomkit-tools-api-") as temporary:
        work = Path(temporary)
        base = options.wheel.resolve() if options.wheel else build(root, work / "base")
        ocr = (
            options.ocr_wheel.resolve()
            if options.ocr_wheel
            else build(root / "backends/rusto/python", work / "ocr", "rusto-python")
        )
        environment = work / "venv"
        run("uv", "venv", "--python", sys.executable, str(environment), cwd=work)
        python = environment / "bin" / "python"
        run(
            "uv",
            "pip",
            "install",
            "--python",
            str(python),
            "--no-deps",
            str(base),
            str(ocr),
            cwd=work,
        )
        writer = str(root / "tests/python/api_signatures.py")
        for package, directory, flags in SNAPSHOTS:
            run(
                str(python), "-I", writer, "write", package, str(root / directory), *flags, cwd=work
            )


if __name__ == "__main__":
    main()
