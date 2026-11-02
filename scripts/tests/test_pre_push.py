"""Tests of .githooks/pre-push, in a scratch repository with a fake `make`.

The hook runs the Rust side of the public API parity gate when the pushed
commits touch what the gate reads. Each test commits in a scratch repository,
feeds the hook the lines git gives it on its standard input, and reads what
the fake `make` recorded: its directory and its targets.
"""

from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

HOOK = Path(__file__).resolve().parents[2] / ".githooks" / "pre-push"
ZERO = "0" * 40


class PrePush(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.repo = self.tmp / "repo"
        self.log = self.tmp / "make.log"
        fake = self.tmp / "make"
        fake.write_text(f'#!/bin/sh\necho "$PWD $*" >> {self.log}\nexit "${{FAKE_STATUS:-0}}"\n')
        fake.chmod(0o755)
        self.env = {
            **os.environ,
            "MAKE": str(fake),
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "Test",
            "GIT_AUTHOR_EMAIL": "test@example.com",
            "GIT_COMMITTER_NAME": "Test",
            "GIT_COMMITTER_EMAIL": "test@example.com",
        }
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.first = self.commit("README.md", "first")

    def tearDown(self):
        self._tmp.cleanup()

    def git(self, *arguments: str, cwd: Path | None = None) -> str:
        return subprocess.run(
            ["git", *arguments],
            cwd=cwd or self.repo,
            env=self.env,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def commit(self, path: str, text: str) -> str:
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        self.git("add", path)
        self.git("commit", "-q", "-m", f"change {path}")
        return self.git("rev-parse", "HEAD")

    def push(self, local: str, remote: str, *, cwd: Path | None = None, status: int = 0):
        """Run the hook as `git push origin` would for one branch."""
        line = f"refs/heads/main {local} refs/heads/main {remote}\n"
        return subprocess.run(
            ["sh", str(HOOK), "origin", "https://example.com/repo.git"],
            cwd=cwd or self.repo,
            env={**self.env, "FAKE_STATUS": str(status)},
            input=line,
            capture_output=True,
            text=True,
        )

    def made(self) -> list[str]:
        return self.log.read_text().splitlines() if self.log.exists() else []

    def test_a_push_touching_no_public_api_runs_nothing(self):
        head = self.commit("guide/notes.md", "notes")
        self.assertEqual(self.push(head, self.first).returncode, 0)
        self.assertEqual(self.made(), [])

    def test_a_push_touching_the_crates_runs_the_gate(self):
        head = self.commit("crates/kit/src/lib.rs", "pub fn f() {}")
        result = self.push(head, self.first)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.made(), [f"{self.repo} --no-print-directory api-test api-parity"])

    def test_each_path_the_gate_reads_runs_it(self):
        for path in (
            "adapters/roomkit/src/lib.rs",
            "python/roomkit_tools/fs.py",
            "api/mapping.toml",
            "scripts/api-parity.py",
            "examples/tests/api_defaults.rs",
            "tests/python/public_api_signatures.json",
            "backends/rusto/python/tests/public_api_signatures.json",
        ):
            with self.subTest(path=path):
                base = self.git("rev-parse", "HEAD")
                head = self.commit(path, path)
                self.log.unlink(missing_ok=True)
                self.assertEqual(self.push(head, base).returncode, 0)
                self.assertEqual(len(self.made()), 1)

    def test_a_new_branch_is_read_from_the_commits_no_remote_has(self):
        head = self.commit("crates/kit/src/lib.rs", "pub fn f() {}")
        self.assertEqual(self.push(head, ZERO).returncode, 0)
        self.assertEqual(len(self.made()), 1)

    def test_the_gates_failure_stops_the_push(self):
        head = self.commit("api/mapping.toml", "[rules]")
        self.assertEqual(self.push(head, self.first, status=1).returncode, 1)

    def test_a_pushed_commit_other_than_the_checked_out_one_is_refused(self):
        pushed = self.commit("crates/kit/src/lib.rs", "pub fn f() {}")
        self.commit("guide/notes.md", "notes")
        result = self.push(pushed, self.first)
        self.assertEqual(result.returncode, 1)
        self.assertIn("not the checked-out commit", result.stderr)
        self.assertEqual(self.made(), [])

    def test_uncommitted_changes_to_what_the_gate_reads_are_refused(self):
        head = self.commit("crates/kit/src/lib.rs", "pub fn f() {}")
        (self.repo / "crates/kit/src/lib.rs").write_text("pub fn g() {}")
        result = self.push(head, self.first)
        self.assertEqual(result.returncode, 1)
        self.assertIn("uncommitted", result.stderr)
        self.assertEqual(self.made(), [])

    def test_in_a_worktree_the_gate_runs_on_the_worktree(self):
        tree = self.tmp / "tree"
        self.git("worktree", "add", "-q", "-b", "topic", str(tree))
        (tree / "crates").mkdir()
        (tree / "crates" / "lib.rs").write_text("pub fn f() {}")
        self.git("add", "crates/lib.rs", cwd=tree)
        self.git("commit", "-q", "-m", "change", cwd=tree)
        head = self.git("rev-parse", "HEAD", cwd=tree)
        result = self.push(head, self.first, cwd=tree)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.made(), [f"{tree} --no-print-directory api-test api-parity"])


if __name__ == "__main__":
    unittest.main()
