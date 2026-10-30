"""Regenerate optional engine notices from the locked Cargo registry sources."""

import json
import subprocess
from pathlib import Path

root = Path(__file__).resolve().parent
metadata = json.loads(
    subprocess.check_output(
        [
            "cargo",
            "metadata",
            "--manifest-path",
            str(root / "Cargo.toml"),
            "--format-version",
            "1",
            "--locked",
        ],
        text=True,
    )
)
parts = [
    "Third-party notices for the optional RustO OCR distribution.\n\n"
    "License texts from locked dependency sources; model weights are not included.\n"
]
for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
    if package["source"] is None:
        continue
    source = Path(package["manifest_path"]).parent
    files = [
        path
        for path in source.iterdir()
        if path.is_file()
        and path.name.lower().startswith(("license", "copying", "copyright", "notice"))
    ]
    if (
        not files
        and package["version"] == "0.27.0"
        and (package["name"] == "rten" or package["name"].startswith("rten-"))
    ):
        files = list((root / "licenses/upstream").glob("rten-0.27.0-*.txt"))
    if not files and (package["name"], package["version"]) == ("flatbuffers", "24.12.23"):
        files = [root / "licenses/upstream/flatbuffers-24.12.23.txt"]
    if not files:
        raise RuntimeError(f"Inspect missing notices for {package['name']} {package['version']}")
    for path in sorted(files):
        parts.append(
            f"\n--- {package['name']}-{package['version']}/{path.name} "
            f"({package['license']}) ---\n\n" + path.read_text()
        )
text = "".join(parts)
(root / "licenses/native_dependencies.txt").write_text(
    "\n".join(line.rstrip() for line in text.splitlines()) + "\n"
)
