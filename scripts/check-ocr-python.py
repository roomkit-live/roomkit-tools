#!/usr/bin/env python3
"""Validate separately installed OCR wheel and sdist; models are optional host input."""

import argparse
import json
import os
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import zipfile
from email.parser import BytesParser
from pathlib import Path

import artifact_files


def run(*args, cwd, env=None):
    subprocess.run(args, cwd=cwd, env=env, check=True)


def main():
    root = Path(__file__).resolve().parents[1]
    # Extracted sdists live outside the checkout's rustup override.
    os.environ.setdefault(
        "RUSTUP_TOOLCHAIN",
        tomllib.loads((root / "rust-toolchain.toml").read_text())["toolchain"]["channel"],
    )
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-wheel", type=Path, help="Validated base package wheel")
    parser.add_argument("--wheel", type=Path, help="Optional OCR backend wheel")
    parser.add_argument("--sdist", type=Path, help="Optional OCR backend source archive")
    options = parser.parse_args()
    supplied = (options.base_wheel, options.wheel, options.sdist)
    if any(supplied) and not all(supplied):
        parser.error("--base-wheel, --wheel and --sdist must be supplied together")
    plugin = root / "backends/rusto/python"
    project = tomllib.loads((plugin / "pyproject.toml").read_text())["project"]
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
        if options.base_wheel:
            base = options.base_wheel.resolve()
            wheel = options.wheel.resolve()
            source = options.sdist.resolve()
        else:
            for directory in [root, plugin]:
                run(
                    *maturin,
                    "build",
                    "--release",
                    "--locked",
                    "--interpreter",
                    sys.executable,
                    "--out",
                    str(artifacts),
                    cwd=directory,
                )
            run(*maturin, "sdist", "--out", str(artifacts), cwd=plugin)
            (base,) = artifacts.glob("roomkit_tools-*.whl")
            (wheel,) = artifacts.glob("roomkit_tools_ocr_rusto-*.whl")
            (source,) = artifacts.glob("*.tar.gz")
        with zipfile.ZipFile(wheel) as archive:
            names = archive.namelist()
            shipped = [n for n in names if n.endswith("/licenses/native_dependencies.txt")]
            assert shipped, names
            # The notices cover every locked crate linked into the extension.
            notices = work / "native_dependencies.txt"
            notices.write_bytes(archive.read(shipped[0]))
            script = str(root / "scripts/native-notices.py")
            run(sys.executable, script, "check", project["name"], str(notices), cwd=root)
            assert any(n.endswith((".so", ".pyd")) for n in names)
            assert not any(n.endswith((".onnx", ".rten")) for n in names)
            metadata = BytesParser().parsebytes(
                archive.read(next(n for n in names if n.endswith("/METADATA")))
            )
            assert metadata["Name"] == project["name"]
            assert metadata["Version"] == project["version"]
            assert metadata["License-Expression"] == "MIT"
            assert (
                metadata.get_payload(decode=True).decode("utf-8").strip()
                == (plugin / "README.md").read_text(encoding="utf-8").strip()
            )
            assert metadata.get_all("Requires-Dist", []) == project["dependencies"]
            assert all(
                any(n.endswith(f".dist-info/licenses/{notice}") for n in names)
                for notice in metadata.get_all("License-File", [])
            )
        # Untracked or ignored local files must reach neither artifact.
        project_path = str(plugin.relative_to(root))
        artifact_files.check_wheel(
            wheel,
            root,
            "roomkit_tools_ocr_rusto",
            project=project_path,
            extra=("licenses/native_dependencies.txt",),
        )
        artifact_files.check_sdist(source, root, project=project_path)
        with tarfile.open(source) as archive:
            assert not any(n.endswith((".onnx", ".rten")) for n in archive.getnames())
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
