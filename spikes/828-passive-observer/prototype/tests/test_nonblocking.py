"""Non-blocking reader suite (spike #828): sources.base.iter_lines + cursor_store.CursorStore.

Every file here is generated at test time from invented values under the scratchpad directory
(SCRATCH below; override with VETTD_SPIKE_SCRATCH). Nothing reads real harness state.

WAL is not applicable to this suite by harness choice (plan D1): the two parsers this prototype
commits to, Claude Code and Codex, write append-only JSONL files, so the reader is a byte-offset
file streamer. Harnesses that keep sessions in a WAL-mode SQLite database (Hermes, OpenClaw's
runtime state, Cursor's state.vscdb) were excluded from v1 exactly because a reader there is a
database client with lock and schema coupling; nothing below says anything about them.

Each test states what it proves and what it cannot prove.
"""
import json
import os
import random
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from cursor_store import CursorStore  # noqa: E402
from sources.base import Cursor, iter_lines  # noqa: E402

PROTO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TESTS_DIR = os.path.dirname(os.path.abspath(__file__))
CHILD = os.path.join(TESTS_DIR, "_reader_child.py")
SCRATCH = os.environ.get(
    "VETTD_SPIKE_SCRATCH",
    "/tmp/claude-0/-home-user/1a612e42-ee29-5c51-a07a-654a25ec2209/scratchpad",
)
MIB = 1024 * 1024
LARGE_TARGET_BYTES = 200 * MIB
MEM_DELTA_LIMIT_BYTES = 64 * MIB
MIN_FREE_BYTES = 1024 * MIB


def record(seq: int, pad: int = 0) -> bytes:
    """One invented ndjson record; `seq` is the record's own identity."""
    return json.dumps({"seq": seq, "kind": "fixture", "pad": "x" * pad}, separators=(",", ":")).encode("ascii") + b"\n"


def write_records(path: str, n: int, pad: int = 0):
    lines = [record(i, pad) for i in range(n)]
    with open(path, "wb") as fh:
        fh.write(b"".join(lines))
    return lines


def line_end_offsets(lines) -> list:
    """Independent reference for iter_lines' offsets: running sum of line lengths."""
    out, total = [], 0
    for ln in lines:
        total += len(ln)
        out.append(total)
    return out


def maxrss_bytes(raw: int) -> int:
    """ru_maxrss is kilobytes on Linux and bytes on macOS."""
    return raw if sys.platform == "darwin" else raw * 1024


class ScratchDirCase(unittest.TestCase):
    def setUp(self):
        os.makedirs(SCRATCH, exist_ok=True)
        self.dir = tempfile.mkdtemp(prefix="nb-", dir=SCRATCH)

    def tearDown(self):
        shutil.rmtree(self.dir, ignore_errors=True)


class RenameWhileOpen(ScratchDirCase):
    def test_rename_while_open(self):
        """Proves: on POSIX an open reader keeps its inode, so renaming the session file mid-read
        (what a harness does on archive/rotate) neither errors nor drops lines: every line and the
        final offset match the original file. Proves POSIX inode semantics only. Cannot prove:
        Windows share-mode behaviour (a rename of an open file can fail there); that needs the
        Rust share_mode test scoped in #965. Also cannot prove anything about a harness truncating
        or rewriting the file in place."""
        path = os.path.join(self.dir, "session-a.ndjson")
        moved = os.path.join(self.dir, "session-a.archived.ndjson")
        lines = write_records(path, 3000)
        got = []
        for i, item in enumerate(iter_lines(path)):
            got.append(item)
            if i == 999:
                os.rename(path, moved)
                self.assertFalse(os.path.exists(path), "rename must have happened mid-iteration")
        self.assertEqual([ln for _, ln in got], lines)
        self.assertEqual([off for off, _ in got], line_end_offsets(lines))
        self.assertEqual(got[-1][0], os.path.getsize(moved))


class KillMidRead(ScratchDirCase):
    LINES = 400
    PER_LINE_SLEEP = 0.003  # full pass ~1.2 s, so every kill delay below lands mid-read

    def _run_child(self, paths, kill_after=None):
        argv = [sys.executable, CHILD, paths["input"], paths["cursor"], paths["output"], str(self.PER_LINE_SLEEP)]
        proc = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if kill_after is None:
            out, err = proc.communicate(timeout=120)
            self.assertEqual(proc.returncode, 0, err)
            return json.loads(out.strip().splitlines()[-1])
        ready = proc.stdout.readline().strip()
        self.assertEqual(ready, "ready", "child must have committed its first record before the kill")
        time.sleep(kill_after)
        proc.kill()
        _, err = proc.communicate(timeout=60)
        self.assertEqual(proc.returncode, -signal.SIGKILL,
                         "child finished before the kill landed (delay %.3fs); %s" % (kill_after, err))
        return None

    def test_kill_mid_read_cursor_consistent(self):
        """Proves, three times with different random kill delays: after a SIGKILL at an arbitrary
        moment the persisted cursor is loadable and sits on a complete-line boundary (temp+rename
        commit is atomic against process death; never a torn store), the restarted reader resumes
        from that cursor and not from zero, re-reads at most the single in-flight line, and the
        concatenated output log equals a single-pass read: no gaps, no duplicates. Cannot prove:
        exactly-once without the consumer-side idempotency key on the record's own seq (an output
        append and a cursor rename are two files and cannot commit together; see _reader_child.py);
        durability across power loss (SIGKILL keeps the page cache, fsync is not what is exercised);
        anything about Windows. Reproduce a run with VETTD_SPIKE_KILL_SEED=<seed>."""
        seed = int(os.environ.get("VETTD_SPIKE_KILL_SEED", time.time_ns() % 1_000_000))
        rng = random.Random(seed)
        delays = [rng.uniform(0.1, 0.6) for _ in range(3)]
        for attempt, delay in enumerate(delays):
            with self.subTest(attempt=attempt, delay=round(delay, 3), seed=seed):
                self._one_kill_cycle(delay, "seed=%d delay=%.3f" % (seed, delay))

    def _one_kill_cycle(self, delay, ctx):
        sub = os.path.join(self.dir, "cycle-%d" % int(delay * 1e6))
        os.makedirs(sub)
        paths = {"input": os.path.join(sub, "session.ndjson"), "cursor": os.path.join(sub, "cursors.json"),
                 "output": os.path.join(sub, "events.log")}
        lines = write_records(paths["input"], self.LINES)
        boundaries = set(line_end_offsets(lines))
        self._run_child(paths, kill_after=delay)
        # The persisted cursor after the kill: loadable, non-zero, on a line boundary, right inode.
        cur = CursorStore(paths["cursor"]).get(paths["input"])
        self.assertIsNotNone(cur, "cursor store unreadable after kill (%s)" % ctx)
        self.assertGreater(cur.byte_offset, 0, ctx)
        self.assertIn(cur.byte_offset, boundaries, "cursor is not on a line boundary (%s)" % ctx)
        self.assertEqual(cur.inode, os.stat(paths["input"]).st_ino, ctx)
        with open(paths["output"], "rb") as fh:
            logged_before = fh.read().count(b"\n")
        self.assertGreater(logged_before, 0, ctx)
        self.assertLess(logged_before, self.LINES, "kill landed after the whole file was read (%s)" % ctx)
        report = self._run_child(paths)
        self.assertEqual(report["start"], cur.byte_offset, "resume did not start at the persisted cursor (%s)" % ctx)
        self.assertLessEqual(report["skipped"], 1, "more than the one in-flight line was replayed (%s)" % ctx)
        self.assertEqual(report["lines"] + logged_before, self.LINES, ctx)
        with open(paths["output"], "rb") as fh:
            logged = [int(x) for x in fh.read().split(b"\n") if x]
        self.assertEqual(logged, list(range(self.LINES)), "gap or duplicate in the event log (%s)" % ctx)
        final = CursorStore(paths["cursor"]).get(paths["input"])
        self.assertEqual(final.byte_offset, os.path.getsize(paths["input"]), ctx)


class BoundedMemoryLargeFile(unittest.TestCase):
    READER = (
        "import sys, resource\n"
        "sys.path.insert(0, sys.argv[1])\n"
        "from sources.base import iter_lines\n"
        "n = 0; last = 0\n"
        "for off, line in iter_lines(sys.argv[2]):\n"
        "    n += 1; last = off\n"
        "print(n, last, resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)\n"
    )
    BASELINE = (
        "import sys, resource\n"
        "sys.path.insert(0, sys.argv[1])\n"
        "from sources.base import iter_lines\n"
        "print(0, 0, resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)\n"
    )

    def setUp(self):
        os.makedirs(SCRATCH, exist_ok=True)
        self.large = os.path.join(SCRATCH, "nb-large-%d.ndjson" % os.getpid())
        self.expected_lines = 0

    def tearDown(self):
        try:
            os.remove(self.large)
        except OSError:
            pass

    def _generate(self):
        block_lines = []
        block = b""
        i = 0
        while len(block) < MIB:
            ln = record(i, 160)
            block_lines.append(ln)
            block += ln
            i += 1
        repeats = -(-LARGE_TARGET_BYTES // len(block))
        with open(self.large, "wb") as fh:
            for _ in range(repeats):
                fh.write(block)
        self.expected_lines = len(block_lines) * repeats

    def _measure(self, code):
        argv = [sys.executable, "-c", code, PROTO_DIR, self.large]
        res = subprocess.run(argv, capture_output=True, text=True, timeout=120)
        self.assertEqual(res.returncode, 0, res.stderr)
        n, last, rss = res.stdout.split()
        return int(n), int(last), maxrss_bytes(int(rss))

    def test_bounded_memory_large_file(self):
        """Proves: streaming a ~200 MB ndjson file through iter_lines in a fresh interpreter grows
        peak RSS by less than 64 MB over an interpreter that only imports the module, while reading
        every line to the final offset, so memory is bounded by line length, not file size. Proves
        streaming in Python only; says nothing about the Rust reader #965 builds. Cannot prove: the
        bound for pathological single lines longer than 64 MB (the contract bounds memory by the
        longest line, and that line would legitimately exceed the limit). Skipped only when the
        scratchpad has under 1 GB free; a skip is reported, never silent."""
        free = shutil.disk_usage(SCRATCH).free
        if free < MIN_FREE_BYTES:
            self.skipTest("scratchpad has %d MB free, under the 1 GB needed for the 200 MB fixture" % (free // MIB))
        self._generate()
        size = os.path.getsize(self.large)
        self.assertGreaterEqual(size, LARGE_TARGET_BYTES)
        started = time.perf_counter()
        _, _, base_rss = self._measure(self.BASELINE)
        n, last, read_rss = self._measure(self.READER)
        elapsed = time.perf_counter() - started
        self.assertEqual(n, self.expected_lines)
        self.assertEqual(last, size)
        delta = read_rss - base_rss
        self.assertLess(delta, MEM_DELTA_LIMIT_BYTES,
                        "peak RSS grew by %d MB reading %d MB in %.1fs" % (delta // MIB, size // MIB, elapsed))


class ByteOffsetResume(ScratchDirCase):
    def test_byte_offset_resume(self):
        """Proves: a persisted end offset resumes exactly at the next line; after appending complete
        lines plus a trailing partial line, resuming yields exactly the appended complete lines and
        never the partial one; once the newline lands, the next resume yields that line exactly once.
        Together: the cursor never points mid-line and a line is never yielded twice or skipped.
        Cannot prove: behaviour when a writer rewrites earlier bytes (append-only is assumed);
        behaviour under inode change (that is the store/source inode check, not iter_lines)."""
        path = os.path.join(self.dir, "session-b.ndjson")
        first = write_records(path, 4)
        head = list(iter_lines(path))
        self.assertEqual([ln for _, ln in head], first)
        mid = head[1][0]  # cursor after two of four lines
        self.assertEqual([ln for _, ln in iter_lines(path, mid)], first[2:])
        cursor = head[-1][0]
        self.assertEqual(cursor, os.path.getsize(path))
        appended = [record(10), record(11), record(12)]
        partial = record(13)[:-1]
        with open(path, "ab") as fh:
            fh.write(b"".join(appended) + partial)
        resumed = list(iter_lines(path, cursor))
        self.assertEqual([ln for _, ln in resumed], appended)
        self.assertNotIn(partial, [ln.rstrip(b"\n") for _, ln in resumed])
        cursor = resumed[-1][0]
        self.assertEqual(cursor, os.path.getsize(path) - len(partial), "cursor must stop before the partial line")
        with open(path, "ab") as fh:
            fh.write(b"\n")
        final = list(iter_lines(path, cursor))
        self.assertEqual([ln for _, ln in final], [record(13)])
        self.assertEqual(final[-1][0], os.path.getsize(path))
        self.assertEqual(list(iter_lines(path, final[-1][0])), [])


class DiskCap(ScratchDirCase):
    def test_disk_cap(self):
        """Proves: with cap_bytes set, save() keeps the store file at or under the cap by evicting
        the least recently updated entries first (a re-set entry is young again), keeps a contiguous
        newest suffix, and that ordering survives a reload from disk. Cannot prove: the cap holds
        when a single entry (or the empty document) is larger than the cap; that degenerate case
        is not a store the prototype configures."""
        cap = 512
        store_path = os.path.join(self.dir, "cursors.json")
        store = CursorStore(store_path, cap_bytes=cap)
        paths = ["/fixture/sessions/s%04d.ndjson" % i for i in range(200)]
        for i, p in enumerate(paths):
            store.set(p, Cursor(path=p, byte_offset=1000 * (i + 1), inode=100 + i))
        store.set(paths[0], Cursor(path=paths[0], byte_offset=1, inode=100))  # refresh the oldest
        store.save()
        self.assertLessEqual(os.path.getsize(store_path), cap)
        kept = list(store.entries())
        self.assertTrue(1 <= len(kept) < len(paths), kept)
        self.assertEqual(kept[-1], paths[0], "the re-set entry must be the newest")
        self.assertEqual(kept[:-1], paths[-(len(kept) - 1):], "survivors must be the newest contiguous suffix")
        self.assertNotIn(paths[1], kept, "the oldest untouched entry must be evicted first")
        reloaded = CursorStore(store_path, cap_bytes=cap)
        self.assertEqual(list(reloaded.entries()), kept)
        self.assertEqual(reloaded.get(paths[0]).byte_offset, 1)
        self.assertEqual([n for n in os.listdir(self.dir) if n.endswith(".tmp")], [], "no temp file left behind")


class Determinism(ScratchDirCase):
    def test_determinism_iter_lines(self):
        """Proves: two reads of the same file yield identical (offset, line) sequences, and those
        offsets equal an independently computed running sum of line lengths, so a persisted cursor
        from one read is meaningful to the next. Cannot prove: determinism across a file that
        changed between reads (append-only growth is the byte-offset test's concern)."""
        path = os.path.join(self.dir, "session-c.ndjson")
        lines = write_records(path, 500, pad=7)
        first = list(iter_lines(path))
        second = list(iter_lines(path))
        self.assertEqual(first, second)
        self.assertEqual([off for off, _ in first], line_end_offsets(lines))
        self.assertEqual([ln for _, ln in first], lines)


class CursorStoreLoad(ScratchDirCase):
    def test_load_tolerates_missing_and_corrupt(self):
        """Proves: a missing file, non-JSON bytes, a JSON document of the wrong shape, and a
        malformed entry each load as empty (or drop just that entry) instead of raising, so a
        damaged local store degrades to a fresh read rather than blocking the collector; and a
        set/save/load round trip preserves byte_offset and inode. Cannot prove: behaviour on a
        store written by a future STORE_VERSION."""
        p = os.path.join(self.dir, "cursors.json")
        self.assertEqual(CursorStore(p).entries(), {})
        with open(p, "wb") as fh:
            fh.write(b"\xff\xfe not json")
        self.assertEqual(CursorStore(p).entries(), {})
        with open(p, "w") as fh:
            json.dump(["not", "an", "object"], fh)
        self.assertEqual(CursorStore(p).entries(), {})
        with open(p, "w") as fh:
            json.dump({"version": 1, "entries": {"/fixture/a.ndjson": {"byte_offset": "12"},
                                                 "/fixture/b.ndjson": {"byte_offset": 12, "inode": 7, "seq": 3}}}, fh)
        store = CursorStore(p)
        self.assertEqual(list(store.entries()), ["/fixture/b.ndjson"])
        store.set("/fixture/c.ndjson", Cursor(path="/fixture/c.ndjson", byte_offset=99, inode=None))
        store.save()
        again = CursorStore(p)
        self.assertEqual(again.get("/fixture/b.ndjson"), Cursor(path="/fixture/b.ndjson", byte_offset=12, inode=7))
        self.assertEqual(again.get("/fixture/c.ndjson"), Cursor(path="/fixture/c.ndjson", byte_offset=99, inode=None))
        self.assertEqual(list(again.entries()), ["/fixture/b.ndjson", "/fixture/c.ndjson"])


if __name__ == "__main__":
    unittest.main()
