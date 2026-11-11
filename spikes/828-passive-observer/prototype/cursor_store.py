"""Local cursor store for resumable, non-blocking session reads (spike #828).

One JSON file maps a session path to the byte offset and inode last read. Offsets always sit on a
line boundary because they come from `sources.base.iter_lines` (a trailing partial line is never
consumed). The file is local-only bookkeeping: paths in it never egress, and the caller chooses
where it lives (`observe.py --cursor-store`), never inside a harness directory.

Guarantees:
- `save()` is atomic against process death: the document is written to a temp file in the same
  directory, fsync'd, then `os.replace`d over the store path, so a reader never sees a torn file.
  It does not guarantee durability across power loss of the *directory* entry (no dir fsync).
- `load` is fail-open: a missing or corrupt file (bad JSON, wrong shape, bad entry) yields an
  empty store rather than an exception; malformed entries are dropped individually.
- `cap_bytes` bounds the serialized size: on `save()`, the oldest entries (by last set/update
  order, persisted as `seq` so age survives reload) are evicted until the document fits.
"""
from __future__ import annotations

import json
import os
import tempfile
from typing import Dict, Optional

from sources.base import Cursor

STORE_VERSION = 1
EMPTY_STORE_SIZE = len(b'{"entries":{},"version":1}\n')


class CursorStore:
    """path -> Cursor map with atomic persistence and an optional on-disk size cap."""

    def __init__(self, path: str, cap_bytes: Optional[int] = None) -> None:
        if cap_bytes is not None and cap_bytes < EMPTY_STORE_SIZE:
            raise ValueError(f"cap_bytes must be >= {EMPTY_STORE_SIZE} or None")
        self.path = path
        self.cap_bytes = cap_bytes
        self._entries: Dict[str, Cursor] = {}  # insertion order == age, oldest first
        self._seq: Dict[str, int] = {}
        self._next_seq = 0
        self._load()

    # ---- reads -------------------------------------------------------------------------------

    def get(self, path: str) -> Optional[Cursor]:
        return self._entries.get(path)

    def entries(self) -> Dict[str, Cursor]:
        """Snapshot of the map, oldest first."""
        return dict(self._entries)

    # ---- writes ------------------------------------------------------------------------------

    def set(self, path: str, cursor: Cursor) -> None:
        """Record `cursor` for `path`; the entry becomes the newest for eviction purposes."""
        stored = Cursor(path=path, byte_offset=cursor.byte_offset, inode=cursor.inode)
        self._entries.pop(path, None)
        self._entries[path] = stored
        self._seq[path] = self._next_seq
        self._next_seq += 1

    def save(self) -> None:
        """Evict beyond `cap_bytes`, then write temp+fsync+rename so the file is never torn."""
        self._evict()
        data = self._serialize()
        directory = os.path.dirname(os.path.abspath(self.path))
        os.makedirs(directory, exist_ok=True)
        fd, tmp = tempfile.mkstemp(prefix=os.path.basename(self.path) + ".", suffix=".tmp", dir=directory)
        try:
            with os.fdopen(fd, "wb") as fh:
                fh.write(data)
                fh.flush()
                os.fsync(fh.fileno())
            os.replace(tmp, self.path)
        except BaseException:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise

    # ---- internals ---------------------------------------------------------------------------

    def _serialize(self) -> bytes:
        entries = {
            path: {"byte_offset": cur.byte_offset, "inode": cur.inode, "seq": self._seq[path]}
            for path, cur in self._entries.items()
        }
        doc = {"version": STORE_VERSION, "entries": entries}
        return (json.dumps(doc, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")

    def _evict(self) -> None:
        if self.cap_bytes is None:
            return
        while self._entries and len(self._serialize()) > self.cap_bytes:
            oldest = next(iter(self._entries))
            del self._entries[oldest]
            del self._seq[oldest]

    def _load(self) -> None:
        try:
            with open(self.path, "rb") as fh:
                doc = json.loads(fh.read().decode("utf-8"))
        except (OSError, ValueError):  # missing, unreadable, not UTF-8, or not JSON -> empty
            return
        if not isinstance(doc, dict) or doc.get("version") != STORE_VERSION:
            return
        raw = doc.get("entries") if isinstance(doc, dict) else None
        if not isinstance(raw, dict):
            return
        rows = []
        for path, row in raw.items():
            parsed = _parse_entry(path, row)
            if parsed is not None:
                rows.append(parsed)
        for seq, path, cur in sorted(rows, key=lambda r: (r[0], r[1])):
            self._entries[path] = cur
            self._seq[path] = seq
        self._next_seq = (max((r[0] for r in rows), default=-1)) + 1


def _is_int(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _parse_entry(path, row):
    """One store row -> (seq, path, Cursor), or None when the row is not trustworthy."""
    if not isinstance(path, str) or not isinstance(row, dict):
        return None
    offset = row.get("byte_offset")
    inode = row.get("inode")
    seq = row.get("seq", 0)
    if not _is_int(offset) or offset < 0:
        return None
    if inode is not None and not _is_int(inode):
        return None
    if not _is_int(seq):
        seq = 0
    return seq, path, Cursor(path=path, byte_offset=offset, inode=inode)
