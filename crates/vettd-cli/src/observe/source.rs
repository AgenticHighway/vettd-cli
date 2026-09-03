//! The harness-neutral [`Source`] trait and the streaming reader beneath it.
//!
//! A source turns one harness's on-disk session logs into [`SessionFacts`]. The
//! reader under it has one job that is harder than it looks: the harness is
//! *writing* these files while we read them, so the reader must never block the
//! writer, never see half a record, and never lose its place. The Python
//! prototype's `iter_lines`
//! (`spikes/828-passive-observer/prototype/sources/base.py:169-199`) is the
//! reference contract and [`LineReader`] reimplements it:
//!
//! * lines are streamed, so memory is bounded by the longest line and never by
//!   the file size — a 200 MiB transcript is read in [`READ_BUFFER_BYTES`]
//!   chunks (`network_evidence::read_file_tail` slurps a whole tail into a
//!   `String`; that shape is deliberately not reused here);
//! * a **partial trailing line is never yielded** and the offset never advances
//!   past it, because the appended bytes of a JSON object are not a JSON object
//!   and a cursor pointing into one would resume mid-record on the next run;
//! * every yielded item carries `end_offset`, one byte past the line's `\n`, so
//!   it is directly persistable as a [`Cursor`] byte offset.
//!
//! The one deliberate extension over the prototype is [`MAX_LINE_BYTES`]: Python
//! bounds memory by "the longest line" with no ceiling, which is a memory bomb
//! for a corrupt or adversarial log. A longer line here is drained to its
//! newline in [`READ_BUFFER_BYTES`] chunks and reported as
//! [`Line::Oversized`] — length only, never content — so the caller counts one
//! parse error and keeps going at a known line boundary.
//!
//! On Windows the file is opened with a share mode that permits the harness to
//! keep writing, renaming and deleting it underneath us; on Unix that is the
//! default behaviour of `open(2)`.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::types::{Cursor, SessionFacts, SessionRef};

/// Largest single line [`LineReader`] will hold in memory: 64 MiB.
///
/// A line at or under this length is yielded whole; a longer one is drained and
/// reported as [`Line::Oversized`]. The limit is on the *line*, not on the file.
pub(crate) const MAX_LINE_BYTES: u64 = 64 * 1024 * 1024;

/// Bytes pulled from the OS per `fill_buf`, and therefore the reader's own
/// working-set floor while draining an oversized line.
const READ_BUFFER_BYTES: usize = 64 * 1024;

// A line is only known to be oversized once the reader is holding all of it, so peak capacity
// while *detecting* one is governed by where `Vec`'s doubling lands. Because the chunk size
// divides the limit exactly, growth stops on the limit itself (64 MiB) instead of overshooting to
// the next power of two (128 MiB). Nothing else enforces that relationship, so assert it: changing
// either constant to a non-dividing pair would quietly double the worst-case working set.
const _: () = assert!(MAX_LINE_BYTES % READ_BUFFER_BYTES as u64 == 0);
const _: () = assert!((MAX_LINE_BYTES / READ_BUFFER_BYTES as u64).is_power_of_two());

/// One harness's session logs, discovered and read.
///
/// Implementations must be non-blocking readers (`base.py:157-167`): open
/// read-only, stream line by line, never hold a file open across calls, and
/// never write into the harness's directories. `discover` returns every session
/// file inside the window — mains and their sub-agent children as separate refs,
/// so each file keeps its own cursor — and `read` projects one of them, resuming
/// from `cursor` when it is still valid for that file (see [`resume_offset`]).
///
/// The returned [`Cursor`] is where the *next* read of that file should start:
/// one byte past the last complete line consumed.
pub(crate) trait Source {
    /// The harness identifier that goes on the wire, e.g. `"claude_code"`.
    fn harness(&self) -> &'static str;

    /// Every session file under `root` whose activity falls inside
    /// `window_days` of `now_ms`.
    fn discover(
        &self,
        root: &Path,
        window_days: u32,
        now_ms: i64,
    ) -> Result<Vec<SessionRef>, String>;

    /// Project one session file into facts, resuming from `cursor` when valid.
    fn read(
        &self,
        r: &SessionRef,
        cursor: Option<&Cursor>,
    ) -> Result<(SessionFacts, Cursor), String>;
}

/// One item from [`LineReader::next_line`].
///
/// Both variants carry `end_offset` — one byte past the line's `\n` — because
/// both advance the cursor: an oversized line is skipped, not re-read forever.
/// [`Line::Oversized`] deliberately carries no bytes: its content was drained
/// without ever being assembled, and its length is all the caller needs to keep
/// `bytes_read` honest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Line {
    /// A complete line, `bytes` including its trailing `\n`.
    Complete { end_offset: u64, bytes: Vec<u8> },
    /// A line longer than [`MAX_LINE_BYTES`], drained rather than buffered.
    Oversized { end_offset: u64, byte_len: u64 },
}

/// A resumable, streaming, byte-offset line reader over one session file.
///
/// Construct with [`LineReader::open`] and pump with [`LineReader::next_line`]
/// until it returns `None`. The reader is *latched*: once it has seen EOF inside
/// an incomplete line it stays finished, so a line the harness completes while
/// this pass is running is left for the next pass rather than being stitched
/// together from two reads.
pub(crate) struct LineReader {
    inner: BufReader<File>,
    path: PathBuf,
    offset: u64,
    max_line_bytes: u64,
    done: bool,
}

impl LineReader {
    /// Opens `path` read-only and seeks to `start`.
    ///
    /// On Windows the share mode is set explicitly to `FILE_SHARE_READ |
    /// FILE_SHARE_WRITE | FILE_SHARE_DELETE` so the reader can never lock the
    /// harness out of its own session log — it must stay free to append to,
    /// rename and delete a file we hold open.
    ///
    /// This is a **pin, not a fix**: `std` already passes exactly these three
    /// bits (`library/std/src/sys/pal/windows/fs.rs` at 1.85.1), so removing the
    /// call would not change today's behaviour. It is spelled out because the
    /// default is an implementation detail we depend on and would otherwise
    /// have no way to notice changing. The constants are written locally rather
    /// than pulled from a `windows-sys` dependency.
    pub(crate) fn open(path: &Path, start: u64) -> Result<LineReader, String> {
        LineReader::open_with_limit(path, start, MAX_LINE_BYTES)
    }

    /// [`LineReader::open`] with an explicit line limit.
    ///
    /// Only the tests pass anything but [`MAX_LINE_BYTES`], and they need to: at
    /// the real limit, peak RSS cannot tell a correct drain (which still buffers
    /// up to the limit before it knows the line is too long) apart from one that
    /// buffered a slightly larger line whole, so the memory claim can only be
    /// made sharp by shrinking the limit rather than growing the fixture.
    fn open_with_limit(path: &Path, start: u64, max_line_bytes: u64) -> Result<LineReader, String> {
        let mut opts = OpenOptions::new();
        opts.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            const FILE_SHARE_WRITE: u32 = 0x0000_0002;
            const FILE_SHARE_DELETE: u32 = 0x0000_0004;
            opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        }
        let mut file = opts
            .open(path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        if start > 0 {
            file.seek(SeekFrom::Start(start))
                .map_err(|e| format!("seek {} to {start}: {e}", path.display()))?;
        }
        Ok(LineReader {
            inner: BufReader::with_capacity(READ_BUFFER_BYTES, file),
            path: path.to_path_buf(),
            offset: start,
            max_line_bytes,
            done: false,
        })
    }

    /// The offset one byte past the last complete line consumed.
    ///
    /// This is the value to persist as [`Cursor::byte_offset`], including when
    /// no line was yielded at all (it is then the `start` the reader opened at).
    pub(crate) fn offset(&self) -> u64 {
        self.offset
    }

    /// The next complete line, or `None` at a clean stop.
    ///
    /// `None` means either end of file or a partial trailing line; in both cases
    /// [`LineReader::offset`] is a line boundary. An I/O error is returned once
    /// and then latches the reader finished.
    pub(crate) fn next_line(&mut self) -> Option<Result<Line, String>> {
        if self.done {
            return None;
        }
        let mut buf: Vec<u8> = Vec::new();
        let mut len: u64 = 0;
        let mut oversized = false;
        loop {
            match self.consume_chunk(&mut buf, &mut len, &mut oversized) {
                Ok(Some(true)) => break,
                Ok(Some(false)) => {}
                Ok(None) => {
                    // EOF inside a line: the partial trailing line is never
                    // yielded and `self.offset` stays on the last boundary.
                    self.done = true;
                    return None;
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
        self.offset += len;
        Some(Ok(if oversized {
            Line::Oversized {
                end_offset: self.offset,
                byte_len: len,
            }
        } else {
            Line::Complete {
                end_offset: self.offset,
                bytes: buf,
            }
        }))
    }

    /// Absorbs one buffered chunk into the line being assembled.
    ///
    /// `Ok(Some(true))` when the chunk ended the line, `Ok(Some(false))` when
    /// more of it is still to come (or the read was interrupted and should be
    /// retried), `Ok(None)` at end of file with no newline in sight. Only the
    /// bytes actually taken are consumed from the buffered reader, so the chunk
    /// after a line's `\n` is still there for the next call.
    fn consume_chunk(
        &mut self,
        buf: &mut Vec<u8>,
        len: &mut u64,
        oversized: &mut bool,
    ) -> Result<Option<bool>, String> {
        let limit = self.max_line_bytes;
        let chunk = match self.inner.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == ErrorKind::Interrupted => return Ok(Some(false)),
            Err(e) => return Err(format!("read {}: {e}", self.path.display())),
        };
        if chunk.is_empty() {
            return Ok(None);
        }
        let (used, at_newline) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => {
                absorb(buf, len, oversized, &chunk[..=i], limit);
                (i + 1, true)
            }
            None => {
                let n = chunk.len();
                absorb(buf, len, oversized, chunk, limit);
                (n, false)
            }
        };
        self.inner.consume(used);
        Ok(Some(at_newline))
    }
}

/// Appends `chunk` to the line being assembled, or drops the whole line once it
/// passes `limit`.
///
/// The buffer is *replaced* rather than cleared when the limit trips, so the
/// capacity already grown for the doomed line is handed back to the allocator
/// immediately: the point of the limit is that peak memory never tracks the
/// oversized line's length.
fn absorb(buf: &mut Vec<u8>, len: &mut u64, oversized: &mut bool, chunk: &[u8], limit: u64) {
    *len += chunk.len() as u64;
    if *oversized {
        return;
    }
    if *len > limit {
        *oversized = true;
        *buf = Vec::new();
        return;
    }
    buf.extend_from_slice(chunk);
}

/// Where a read of `path` should start, given a persisted `cursor`
/// (`claude_code.py:583-588`).
///
/// Returns 0 — a full re-read — unless the cursor demonstrably still describes
/// this file: same path, an offset that is not past the current end, and, where
/// the platform reports inodes, the same inode. A rotated or recreated file
/// therefore reads from the top rather than resuming at an offset that means
/// something different in it.
///
/// **What this does not catch:** a file truncated and rewritten *in place* to at
/// least the cursor's offset keeps its inode and passes the size test, so the
/// stale offset is accepted and the rewritten prefix is skipped silently. That
/// is faithful to `_resume_offset`, which has the identical two tests, and no
/// harness rewrites a transcript in place — but it is a property this function
/// lacks rather than one it provides, and catching it would need a size or mtime
/// generation stored alongside the offset.
///
/// A cursor carrying `Some(inode)` on a platform that reports none (Windows) is
/// trusted on size alone: the inode check can only ever *reject*, and refusing
/// every cursor there would turn resume off entirely.
pub(crate) fn resume_offset(cursor: Option<&Cursor>, path: &Path, meta: &Metadata) -> u64 {
    let Some(cursor) = cursor else {
        return 0;
    };
    if cursor.path.as_path() != path || cursor.byte_offset > meta.len() {
        return 0;
    }
    match (cursor.inode, inode_of(meta)) {
        (Some(want), Some(have)) if want != have => 0,
        _ => cursor.byte_offset,
    }
}

/// The file's inode, where the platform has one.
///
/// `Some` on Unix, `None` everywhere else, which makes cursor validity
/// size-only on Windows. Callers building a [`Cursor`] store whatever this
/// returns, so a cursor written on one platform is never *wrongly* accepted on
/// another — only, at worst, less strictly checked.
#[cfg(unix)]
pub(crate) fn inode_of(meta: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.ino())
}

/// The file's inode, where the platform has one — always `None` off Unix.
#[cfg(not(unix))]
pub(crate) fn inode_of(_meta: &Metadata) -> Option<u64> {
    None
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
