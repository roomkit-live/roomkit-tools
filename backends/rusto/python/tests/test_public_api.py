"""The installed OCR package's public API against its committed snapshot.

The snapshot is the reference the Rust OCR backend crate is compared with
(``make api-parity``); ``make api-snapshot`` rewrites it in the commit that
changes the public surface.
"""

import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
# The snapshot module lives with the base package's tests in this repository.
sys.path.insert(0, str(HERE.parents[3] / "tests" / "python"))

import api_signatures  # noqa: E402


class SnapshotTests(unittest.TestCase):
    def test_the_snapshot_holds_the_whole_public_surface(self):
        package = api_signatures.Package("roomkit_tools_ocr_rusto")
        drift = api_signatures.drift(HERE, package, with_contracts=False)
        self.assertEqual(drift, [], "run make api-snapshot and commit the rewritten files")


if __name__ == "__main__":
    unittest.main()
