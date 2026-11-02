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


def scratch_env() -> dict[str, str]:
    """The environment without git's own variables. The hook runs these tests
    (through `make api-test`), and git exports GIT_DIR, GIT_WORK_TREE or
    GIT_CONFIG_PARAMETERS to a hook: inherited, they would aim each git
    command of a test at the repository being pushed instead of the scratch
    one."""
    return {name: value for name, value in os.environ.items() if not name.startswith("GIT_")}


class Isolation(unittest.TestCase):
    """The hook tests, run with the variables git exports to a hook, leave the
    repository those variables name as it was."""

    def test_the_repository_a_hook_runs_in_is_never_touched(self):
        with tempfile.TemporaryDirectory() as tmp:
            outer = Path(tmp) / "outer"
            linked = Path(tmp) / "linked"
            self.outer_repository(outer, linked)
            head = self.outer_git(outer, "rev-parse", "HEAD")
            hooks = {
                "a work tree": {
                    "GIT_DIR": str(outer / ".git"),
                    "GIT_WORK_TREE": str(outer),
                    "GIT_INDEX_FILE": str(outer / ".git" / "index"),
                    "GIT_CONFIG_PARAMETERS": "'core.hookspath=/nonexistent'",
                },
                # What git exports to a hook run in a linked worktree: a
                # `git init` given it alone marks the shared repository bare.
                "a linked worktree": {"GIT_DIR": str(outer / ".git" / "worktrees" / "linked")},
            }
            for name, variables in hooks.items():
                with self.subTest(hook=name):
                    result = subprocess.run(
                        ["python3", "-m", "unittest", "-q", "test_pre_push.PrePush"],
                        cwd=Path(__file__).parent,
                        env={**self.quiet(), **variables},
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(self.outer_git(outer, "rev-parse", "HEAD"), head)
                    self.assertEqual(self.outer_git(outer, "status", "--porcelain"), "")
                    self.assertEqual(self.outer_git(outer, "tag", "-l"), "")
                    self.assertEqual(self.outer_git(outer, "config", "core.bare"), "false")
                    self.assertEqual(result.returncode, 0, result.stderr[-2000:])

    @staticmethod
    def quiet() -> dict[str, str]:
        return {**scratch_env(), "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1"}

    def outer_git(self, outer: Path, *arguments: str) -> str:
        identity = ["-c", "user.name=Outer", "-c", "user.email=outer@example.com"]
        return subprocess.run(
            ["git", *identity, *arguments],
            cwd=outer,
            env=self.quiet(),
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

    def outer_repository(self, outer: Path, linked: Path) -> None:
        """A repository with one commit and a linked worktree."""
        outer.mkdir()
        self.outer_git(outer, "init", "-q", "-b", "main")
        (outer / "README.md").write_text("outer\n")
        self.outer_git(outer, "add", "README.md")
        self.outer_git(outer, "commit", "-q", "-m", "outer")
        self.outer_git(outer, "worktree", "add", "-q", str(linked), "-b", "linked")


class PrePush(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = Path(self._tmp.name)
        self.repo = self.tmp / "repo"
        self.log = self.tmp / "make.log"
        fake = self.tmp / "make"
        fake.write_text(
            f'#!/bin/sh\necho "$PWD $*" >> {self.log}\n'
            'case "$*" in *api-adapter-check*) exit "${FAKE_ADAPTER_STATUS:-0}";; esac\n'
            'exit "${FAKE_STATUS:-0}"\n'
        )
        fake.chmod(0o755)
        self.env = {
            **scratch_env(),
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
        return self.push_lines(
            f"refs/heads/main {local} refs/heads/main {remote}\n", cwd=cwd, status=status
        )

    def push_lines(
        self, lines: str, *, cwd: Path | None = None, status: int = 0, adapter_status: int = 0
    ):
        """Run the hook with the lines git gives it, one per pushed ref."""
        return subprocess.run(
            ["sh", str(HOOK), "origin", "https://example.com/repo.git"],
            cwd=cwd or self.repo,
            env={
                **self.env,
                "FAKE_STATUS": str(status),
                "FAKE_ADAPTER_STATUS": str(adapter_status),
            },
            input=lines,
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
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "Makefile",
        ):
            with self.subTest(path=path):
                base = self.git("rev-parse", "HEAD")
                head = self.commit(path, path)
                self.log.unlink(missing_ok=True)
                self.assertEqual(self.push(head, base).returncode, 0)
                gate = f"{self.repo} --no-print-directory api-test api-parity"
                self.assertEqual(self.made()[:1], [gate])

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

    def test_untracked_files_where_the_gate_reads_are_refused(self):
        head = self.commit("crates/kit/src/lib.rs", "pub fn f() {}")
        (self.repo / "crates/kit/src/new.rs").write_text("pub fn g() {}")
        result = self.push(head, self.first)
        self.assertEqual(result.returncode, 1)
        self.assertIn("untracked", result.stderr)
        self.assertEqual(self.made(), [])

    def test_a_dirty_root_manifest_is_refused(self):
        self.commit("Cargo.toml", "[workspace]\n")
        head = self.commit("crates/kit/src/lib.rs", "pub fn f() {}")
        (self.repo / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
        result = self.push(head, self.first)
        self.assertEqual(result.returncode, 1)
        self.assertIn("uncommitted", result.stderr)

    def test_an_annotated_tag_of_the_checked_out_commit_runs_the_gate(self):
        head = self.commit("crates/kit/src/lib.rs", "pub fn f() {}")
        self.git("tag", "-a", "v1", "-m", "release")
        tag = self.git("rev-parse", "v1")
        self.assertNotEqual(tag, head)
        result = self.push_lines(f"refs/tags/v1 {tag} refs/tags/v1 {ZERO}\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.made(), [f"{self.repo} --no-print-directory api-test api-parity"])

    def test_a_deletion_runs_nothing(self):
        result = self.push_lines(f"(delete) {ZERO} refs/heads/old {self.first}\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.made(), [])

    def test_several_refs_run_the_gate_once_or_refuse_another_commit(self):
        head = self.commit("crates/kit/src/lib.rs", "pub fn f() {}")
        self.git("tag", "v1")
        lines = (
            f"refs/heads/main {head} refs/heads/main {self.first}\n"
            f"refs/tags/v1 {head} refs/tags/v1 {ZERO}\n"
        )
        self.assertEqual(self.push_lines(lines).returncode, 0)
        self.assertEqual(len(self.made()), 1)
        self.log.unlink()
        older = head
        newer = self.commit("crates/kit/src/lib.rs", "pub fn g() {}")
        lines = (
            f"refs/heads/main {newer} refs/heads/main {self.first}\n"
            f"refs/heads/topic {older} refs/heads/topic {ZERO}\n"
        )
        result = self.push_lines(lines)
        self.assertEqual(result.returncode, 1)
        self.assertIn("refs/heads/topic", result.stderr)
        self.assertEqual(self.made(), [])

    def test_the_adapter_runs_its_surface_check(self):
        head = self.commit("adapters/roomkit/src/lib.rs", "pub struct Read;")
        result = self.push(head, self.first)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            self.made(),
            [
                f"{self.repo} --no-print-directory api-test api-parity",
                f"{self.repo} --no-print-directory api-adapter-check",
            ],
        )
        self.log.unlink()
        result = self.push_lines(
            f"refs/heads/main {head} refs/heads/main {self.first}\n", adapter_status=2
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("api-adapter-check", result.stderr)
        self.assertIn("roomkit-rs", result.stderr)

    def test_each_path_the_adapters_files_come_from_runs_its_check(self):
        check = f"{self.repo} --no-print-directory api-adapter-check"
        for path in (
            "api/roomkit-adapter-surface.json",
            "api/roomkit-adapter-hidden.json",
            "scripts/rs-surface.py",
            "rust-toolchain.toml",
        ):
            with self.subTest(path=path):
                base = self.git("rev-parse", "HEAD")
                head = self.commit(path, path)
                self.log.unlink(missing_ok=True)
                self.assertEqual(self.push(head, base).returncode, 0)
                self.assertEqual(self.made()[1:], [check])

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
