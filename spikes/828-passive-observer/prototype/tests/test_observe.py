"""End-to-end tests for observe.py cursor and run-record semantics.

The fixture trees are copied before each test. Nothing reads a real harness home, and every output
is written below the test's temporary directory.
"""
import contextlib
import io
import json
import os
import shutil
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import observe  # noqa: E402
from cursor_store import CursorStore  # noqa: E402
from sources.claude_code import ClaudeCodeSource  # noqa: E402
from sources.codex import CodexSource  # noqa: E402

PROTO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURES = os.path.join(PROTO_DIR, "fixtures")
NOW_MS = 1_800_000_000_000
TODAY = "2027-01-15"


class PipelineResumeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.secret = os.path.join(self.tmp.name, "observer-secret")
        with open(self.secret, "wb") as fh:
            fh.write(b"invented-observer-secret-material")

    def tearDown(self):
        self.tmp.cleanup()

    def _copy_root(self, harness):
        source_name = "claude_home" if harness == "claude_code" else "codex_home"
        root = os.path.join(self.tmp.name, source_name)
        shutil.copytree(os.path.join(FIXTURES, source_name), root)
        return root

    def _run(self, harness, root, cursor_path, output_name):
        out = os.path.join(self.tmp.name, output_name)
        argv = [
            "--harness", harness,
            "--root", root,
            "--task", "exercise passive observer resume",
            "--secret-file", self.secret,
            "--out", out,
            "--today", TODAY,
            "--now-ms", str(NOW_MS),
            "--window-days", "3650",
            "--cursor-store", cursor_path,
            "--scrub",
        ]
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            rc = observe.main(argv)
        self.assertEqual(rc, 0, stderr.getvalue())
        with open(out, encoding="utf-8") as fh:
            return json.load(fh)

    def test_unchanged_resume_emits_silence_and_cursors_every_file(self):
        """Proves: after a successful first pass, an unchanged second pass emits zero records and
        marks zero sessions emitted. Every discovered main and child has a persisted cursor, so a
        child is not reread and folded into a partial replacement of its parent's run.

        Cannot prove: behaviour while a harness is concurrently appending; the non-blocking suite
        covers line-boundary cursor safety separately.
        """
        for harness, source_type in (("claude_code", ClaudeCodeSource), ("codex", CodexSource)):
            with self.subTest(harness=harness):
                root = self._copy_root(harness)
                cursor_path = os.path.join(self.tmp.name, harness + "-cursors.json")
                first = self._run(harness, root, cursor_path, harness + "-first.json")
                self.assertGreater(len(first["records"]), 0)

                source = source_type(root)
                refs = source.discover(root, 3650, NOW_MS)
                self.assertEqual(set(CursorStore(cursor_path).entries()), {ref.path for ref in refs})

                resumed = self._run(harness, root, cursor_path, harness + "-resumed.json")
                self.assertEqual(resumed["records"], [])
                self.assertEqual(resumed["coverage"]["sessions_seen"],
                                 sum(ref.kind == "main" for ref in refs))
                self.assertEqual(resumed["coverage"]["sessions_emitted"], 0)
                self.assertEqual(resumed["coverage"]["lines_seen"], 0)
                self.assertEqual(resumed["coverage"]["bytes_read"], 0)
                self.assertEqual(resumed["coverage"]["cursor_state"], "resumed")

    def test_changed_child_rebuilds_the_complete_parent_run(self):
        """Proves: a new complete line in a Claude child triggers one cumulative parent record,
        not a child-only delta. An unknown line changes no signals, so the rebuilt record must have
        exactly the original run id, counts, assets, token totals and BOM.

        Cannot prove: how a production server handles the idempotent replacement.
        """
        harness = "claude_code"
        root = self._copy_root(harness)
        cursor_path = os.path.join(self.tmp.name, "claude-cursors.json")
        first = self._run(harness, root, cursor_path, "claude-first.json")
        self.assertEqual(len(first["records"]), 1)
        original = first["records"][0]

        source = ClaudeCodeSource(root)
        refs = source.discover(root, 3650, NOW_MS)
        child = next(ref for ref in refs if ref.kind == "child")
        with open(child.path, "ab") as fh:
            fh.write(b'{"type":"invented_unknown"}\n')

        changed = self._run(harness, root, cursor_path, "claude-child-changed.json")
        self.assertEqual(len(changed["records"]), 1)
        rebuilt = changed["records"][0]
        for field in ("run_id", "counts", "assets", "tokens", "tokens_by_model", "bom_version"):
            self.assertEqual(rebuilt[field], original[field], field)
        self.assertEqual(changed["coverage"]["sessions_emitted"], 1)

    def test_changed_main_rebuilds_the_complete_run(self):
        """Proves: a new complete line in a main transcript produces one cumulative replacement
        with the prior run facts intact. It never emits the appended line as a zero-context delta.

        Cannot prove: a cross-process append racing the probe and full rebuild.
        """
        harness = "codex"
        root = self._copy_root(harness)
        cursor_path = os.path.join(self.tmp.name, "codex-cursors.json")
        first = self._run(harness, root, cursor_path, "codex-first.json")
        originals = {record["run_id"]: record for record in first["records"]}

        source = CodexSource(root)
        refs = source.discover(root, 3650, NOW_MS)
        main = next(ref for ref in refs if ref.kind == "main")
        with open(main.path, "ab") as fh:
            fh.write(b'{"type":"invented_unknown"}\n')

        changed = self._run(harness, root, cursor_path, "codex-main-changed.json")
        self.assertEqual(len(changed["records"]), 1)
        rebuilt = changed["records"][0]
        original = originals[rebuilt["run_id"]]
        for field in ("counts", "assets", "tokens", "tokens_by_model", "bom_version"):
            self.assertEqual(rebuilt[field], original[field], field)
        self.assertEqual(changed["coverage"]["sessions_emitted"], 1)
        self.assertGreater(changed["coverage"]["bytes_read"], os.path.getsize(main.path),
                           "resource accounting must include the incremental probe and cumulative reread")

    def test_failed_rebuild_does_not_advance_the_probe_cursor(self):
        """Proves: a probe may observe an appended line, but its cursor is not committed when the
        cumulative reread fails. The next collection can retry that line instead of losing it.

        Cannot prove: process death between payload and cursor-store renames; the kill suite covers
        the cursor store's own atomic write boundary.
        """
        harness = "codex"
        root = self._copy_root(harness)
        cursor_path = os.path.join(self.tmp.name, "codex-cursors.json")
        self._run(harness, root, cursor_path, "codex-first.json")

        source = CodexSource(root)
        refs = source.discover(root, 3650, NOW_MS)
        main = next(ref for ref in refs if ref.kind == "main")
        before = CursorStore(cursor_path).get(main.path)
        with open(main.path, "ab") as fh:
            fh.write(b'{"type":"invented_unknown"}\n')

        original_read = CodexSource.read

        def fail_full_read(instance, ref, cursor=None):
            if ref.path == main.path and cursor is None:
                raise OSError("invented full-read failure")
            return original_read(instance, ref, cursor)

        with mock.patch.object(CodexSource, "read", fail_full_read):
            failed = self._run(harness, root, cursor_path, "codex-failed.json")
        self.assertEqual(failed["records"], [])
        self.assertEqual(failed["coverage"]["sessions_skipped_unparseable"], 1)
        after = CursorStore(cursor_path).get(main.path)
        self.assertEqual(after.byte_offset, before.byte_offset)
        self.assertEqual(after.inode, before.inode)

    def test_failed_child_rebuild_preserves_the_complete_parent_record(self):
        """Proves: if a changed child cannot be reread, the observer emits no parent-only record
        and advances neither cursor. A later run can retry the complete group.

        Cannot prove: recovery from a permanently malformed child; coverage reports the skip.
        """
        harness = "claude_code"
        root = self._copy_root(harness)
        cursor_path = os.path.join(self.tmp.name, "claude-cursors.json")
        self._run(harness, root, cursor_path, "claude-first.json")

        source = ClaudeCodeSource(root)
        refs = source.discover(root, 3650, NOW_MS)
        main = next(ref for ref in refs if ref.kind == "main")
        child = next(ref for ref in refs if ref.kind == "child")
        before = CursorStore(cursor_path).entries()
        with open(child.path, "ab") as fh:
            fh.write(b'{"type":"invented_unknown"}\n')

        original_read = ClaudeCodeSource.read

        def fail_child_full_read(instance, ref, cursor=None):
            if ref.path == child.path and cursor is None:
                raise OSError("invented child full-read failure")
            return original_read(instance, ref, cursor)

        with mock.patch.object(ClaudeCodeSource, "read", fail_child_full_read):
            failed = self._run(harness, root, cursor_path, "claude-failed-child.json")
        self.assertEqual(failed["records"], [])
        self.assertEqual(failed["coverage"]["sessions_skipped_unparseable"], 1)
        after = CursorStore(cursor_path).entries()
        for ref in (main, child):
            self.assertEqual(after[ref.path].byte_offset, before[ref.path].byte_offset)
            self.assertEqual(after[ref.path].inode, before[ref.path].inode)


class WindowsStdioTests(unittest.TestCase):
    def test_windows_streams_are_reconfigured_for_unicode_output(self):
        """Proves: the Windows entry point requests UTF-8 with replacement fallback on both output
        streams, so argparse help and ranking glyphs do not fail under a legacy code page.

        Cannot prove: how a particular terminal renders the resulting UTF-8 bytes.
        """
        class Stream:
            def __init__(self):
                self.calls = []

            def reconfigure(self, **kwargs):
                self.calls.append(kwargs)

        stdout, stderr = Stream(), Stream()
        with mock.patch.object(observe.os, "name", "nt"), \
                mock.patch.object(observe.sys, "stdout", stdout), \
                mock.patch.object(observe.sys, "stderr", stderr):
            observe._configure_stdio()
        self.assertEqual(stdout.calls, [{"encoding": "utf-8", "errors": "replace"}])
        self.assertEqual(stderr.calls, [{"encoding": "utf-8", "errors": "replace"}])


class EntryPointInputTests(unittest.TestCase):
    def test_secret_bytes_are_loaded_exactly(self):
        """Proves: leading and trailing bytes that bytes.strip recognises as whitespace remain part
        of the random HMAC key. Cannot prove: secure generation or filesystem permissions."""
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "secret")
            secret = b"\n" + b"invented-secret-material" + b"\t"
            with open(path, "wb") as fh:
                fh.write(secret)
            self.assertEqual(observe._load_secret(path), secret)

    def test_explicit_zero_now_is_not_replaced_by_wall_clock(self):
        """Proves: zero is a valid injected collector timestamp and yields the Unix epoch day.
        Cannot prove: platform datetime behaviour outside Python's supported epoch range."""
        with tempfile.TemporaryDirectory() as tmp:
            root = os.path.join(tmp, "codex_home")
            os.makedirs(root)
            secret = os.path.join(tmp, "secret")
            out = os.path.join(tmp, "out.json")
            with open(secret, "wb") as fh:
                fh.write(b"invented-observer-secret-material")
            argv = [
                "--harness", "codex", "--root", root,
                "--task", "exercise epoch timestamp", "--secret-file", secret,
                "--out", out, "--now-ms", "0", "--window-days", "3660", "--scrub",
            ]
            stdout, stderr = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                rc = observe.main(argv)
            self.assertEqual(rc, 0, stderr.getvalue())
            with open(out, encoding="utf-8") as fh:
                payload = json.load(fh)
            self.assertEqual(payload["emitted_day"], "1970-01-01")


if __name__ == "__main__":
    unittest.main()
