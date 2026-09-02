"""Subprocess helper for tests/test_nonblocking.py (spike #828). Not a test module.

Reads an invented ndjson file line by line with `sources.base.iter_lines`, appends each record's
own `seq` to an output log, and commits the cursor with `CursorStore.save()` after every line. It is
built to be SIGKILLed at a random moment and restarted:

- Resume offset comes from the persisted cursor when its inode still matches the file; otherwise 0.
- Commit order per line is output-append THEN cursor-save, so a kill between the two leaves the
  cursor one line behind the log, never ahead of it (no gap is possible, one replay is).
- Consumption is idempotent on the record's own `seq`: on restart the child skips any record whose
  seq is <= the last seq already in the log, which absorbs that single in-flight replay. This is
  the same shape as the product design, where `run_id` is the server-side idempotency key.

Usage: _reader_child.py <input.ndjson> <cursor.json> <output.log> <per_line_sleep_s>
stdout: the line "ready" once the first record of this run has been committed, then a final JSON
report {"start": <resume offset>, "lines": <records appended>, "skipped": <replays absorbed>}.
"""
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from cursor_store import CursorStore  # noqa: E402
from sources.base import Cursor, iter_lines  # noqa: E402


def last_logged_seq(output_path: str) -> int:
    """Highest seq already committed to the output log; -1 when the log is empty or absent.
    Only complete lines count, so a torn trailing line (not expected: each append is one small
    write) is treated as unwritten."""
    try:
        with open(output_path, "rb") as fh:
            data = fh.read()
    except OSError:
        return -1
    complete = data[: data.rfind(b"\n") + 1] if b"\n" in data else b""
    lines = [ln for ln in complete.split(b"\n") if ln]
    return int(lines[-1]) if lines else -1


def resume_offset(store: CursorStore, input_path: str, inode: int) -> int:
    cur = store.get(input_path)
    if cur is None or cur.inode != inode:
        return 0
    return cur.byte_offset


def main(argv) -> int:
    input_path, cursor_path, output_path, sleep_s = argv[1], argv[2], argv[3], float(argv[4])
    inode = os.stat(input_path).st_ino
    store = CursorStore(cursor_path)
    start = resume_offset(store, input_path, inode)
    floor = last_logged_seq(output_path)
    appended = skipped = 0
    with open(output_path, "ab") as out:
        for end_offset, line in iter_lines(input_path, start):
            seq = json.loads(line)["seq"]
            if seq <= floor:
                skipped += 1
                continue
            out.write(("%d\n" % seq).encode("ascii"))
            out.flush()
            store.set(input_path, Cursor(path=input_path, byte_offset=end_offset, inode=inode))
            store.save()
            appended += 1
            if appended == 1:
                print("ready", flush=True)
            time.sleep(sleep_s)
    print(json.dumps({"start": start, "lines": appended, "skipped": skipped}), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
