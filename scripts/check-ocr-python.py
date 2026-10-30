#!/usr/bin/env python3
"""Validate separately installed OCR wheel and sdist; models are optional host input."""

import json
import os
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from pathlib import Path


def run(*args, cwd, env=None):
    subprocess.run(args, cwd=cwd, env=env, check=True)


def main():
    root = Path(__file__).resolve().parents[1]
    # Extracted sdists live outside the checkout's rustup override.
    os.environ.setdefault(
        "RUSTUP_TOOLCHAIN",
        tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"],
    )
    plugin = root / "backends/rusto/python"
    # The base bridge must never pull in the heavy optional recognition engine.
    base_tree = subprocess.check_output(
        ["cargo", "tree", "--manifest-path", "bindings/python/Cargo.toml", "--locked"],
        cwd=root,
        text=True,
    )
    assert "rusto-rs" not in base_tree and "rten v" not in base_tree
    with tempfile.TemporaryDirectory(prefix="roomkit-ocr-package-") as temporary:
        work = Path(temporary)
        artifacts = work / "artifacts"
        maturin = ["uv", "tool", "run", "--from", "maturin>=1.15,<2", "maturin"]
        for project in [root, plugin]:
            run(
                *maturin,
                "build",
                "--release",
                "--locked",
                "--interpreter",
                sys.executable,
                "--out",
                str(artifacts),
                cwd=project,
            )
        run(*maturin, "sdist", "--out", str(artifacts), cwd=plugin)
        (base,) = artifacts.glob("roomkit_tools-*.whl")
        (wheel,) = artifacts.glob("roomkit_tools_ocr_rusto-*.whl")
        (source,) = artifacts.glob("*.tar.gz")
        with zipfile.ZipFile(wheel) as archive:
            names = archive.namelist()
            assert any(n.endswith("/licenses/native_dependencies.txt") for n in names)
            assert any(n.endswith((".so", ".pyd")) for n in names)
            assert not any(n.endswith((".onnx", ".rten")) for n in names)
        with tarfile.open(source) as archive:
            names = archive.getnames()
            assert not any(
                "/docs/" in n or "bakery" in n or n.endswith("/AGENTS.md") for n in names
            )
        reference = work / "rust.json"
        env = {**os.environ, "ROOMKIT_OCR_SOURCE": str(root)}
        models = os.getenv("ROOMKIT_OCR_MODELS")
        if models:
            result = subprocess.check_output(
                [
                    "cargo",
                    "run",
                    "--release",
                    "--quiet",
                    "--locked",
                    "-p",
                    "roomkit-tools-ocr-rusto",
                    "--example",
                    "recognize",
                    "--",
                    str(Path(models).resolve()),
                    str(root / "tests/fixtures/ocr"),
                    "page.png",
                ],
                cwd=root,
            )
            json.loads(result)  # Require an actual complete native envelope.
            reference.write_bytes(result)
            env["ROOMKIT_OCR_REFERENCE"] = str(reference)
        for label, package in [("wheel", wheel), ("sdist", source)]:
            environment = work / label
            run("uv", "venv", "--python", sys.executable, str(environment), cwd=work)
            python = environment / "bin/python"
            run(
                "uv",
                "pip",
                "install",
                "--python",
                str(python),
                str(base),
                str(package),
                "roomkit==0.110.0",
                cwd=work,
            )
            run(
                str(python),
                "-X",
                "faulthandler",
                "-I",
                "-m",
                "unittest",
                "discover",
                "-s",
                str(plugin / "tests"),
                "-v",
                cwd=work,
                env=env,
            )
            if models:
                run(
                    str(python),
                    "-I",
                    str(root / "examples/roomkit_ocr.py"),
                    "--models",
                    str(Path(models).resolve()),
                    "--image",
                    str(root / "tests/fixtures/ocr/page.png"),
                    cwd=work,
                )


if __name__ == "__main__":
    main()
