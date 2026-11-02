"""Tests of the workspace manifest the published crates are built from."""

from __future__ import annotations

import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class Requirements(unittest.TestCase):
    def test_no_dependency_is_pinned_to_one_version(self):
        """A published crate requires a compatible version (`0.3.2`, `^0.3.2`),
        never an exact one (`=0.3.2`), which would force it on every host;
        Cargo.lock keeps the version built and tested."""
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
        pinned = {
            name: spec
            for name, spec in manifest["workspace"]["dependencies"].items()
            if str(spec.get("version", "") if isinstance(spec, dict) else spec).startswith("=")
        }
        self.assertEqual(pinned, {})


if __name__ == "__main__":
    unittest.main()
