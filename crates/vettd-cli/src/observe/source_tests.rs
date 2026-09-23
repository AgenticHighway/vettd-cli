//! Reader tests, ported from the prototype's non-blocking suite
//! (`spikes/828-passive-observer/prototype/tests/test_nonblocking.py`).
//!
//! Every file here is generated at test time from invented values in a
//! `tempfile` scratch directory; nothing reads real harness state and nothing
//! writes into the repository's fixture tree.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;

const MIB: u64 = 1024 * 1024;

/// One invented ndjson record; `seq` is the record's own identity.
fn record(seq: u64, pad: usize) -> Vec<u8> {
    format!(
        "{{\"seq\":{seq},\"kind\":\"fixture\",\"pad\":\"{}\"}}\n",
        "x".repeat(pad)
    )
    .into_bytes()
}

/// Writes `n` records to `path`, returning them in order.
fn write_records(path: &Path, n: u64, pad: usize) -> Vec<Vec<u8>> {
    let lines: Vec<Vec<u8>> = (0..n).map(|i| record(i, pad)).collect();
    let mut fh = BufWriter::new(File::create(path).expect("create fixture"));
    for line in &lines {
        fh.write_all(line).expect("write fixture");
    }
    fh.flush().expect("flush fixture");
    lines
}

fn append(path: &Path, bytes: &[u8]) {
    let mut fh = OpenOptions::new().append(true).open(path).expect("append");
    fh.write_all(bytes).expect("append bytes");
}

/// Independent reference for the reader's offsets: a running sum of line lengths.
fn end_offsets(lines: &[Vec<u8>]) -> Vec<u64> {
    let mut total = 0u64;
    lines
        .iter()
        .map(|l| {
            total += l.len() as u64;
            total
        })
        .collect()
}

/// Drains a reader, asserting every item is a complete line.
fn read_all(path: &Path, start: u64) -> Vec<(u64, Vec<u8>)> {
    let mut reader = LineReader::open(path, start).expect("open");
    let mut out = Vec::new();
    while let Some(item) = reader.next_line() {
        match item.expect("read") {
            Line::Complete { end_offset, bytes } => out.push((end_offset, bytes)),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(
        reader.offset(),
        out.last().map(|(o, _)| *o).unwrap_or(start),
        "offset() must equal the last yielded end_offset"
    );
    out
}

fn scratch() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.ndjson");
    (dir, path)
}

fn size_of(path: &Path) -> u64 {
    std::fs::metadata(path).expect("stat").len()
}

/// Peak resident set size of this process, in bytes.
///
/// `None` when the platform does not report it. This is a *high-water mark*: a
/// delta across a section proves the section did not push a new peak, which is
/// exactly the one-way claim the memory tests make.
#[cfg(target_os = "linux")]
fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

/// Writes one line of `body_len` `x` bytes plus its newline, in 1 MiB chunks so
/// that generating the fixture does not itself hold the whole line in memory.
fn write_long_line(fh: &mut BufWriter<File>, body_len: u64) {
    let chunk = vec![b'x'; MIB as usize];
    let mut left = body_len;
    while left > 0 {
        let take = left.min(MIB) as usize;
        fh.write_all(&chunk[..take]).expect("write long line");
        left -= take as u64;
    }
    fh.write_all(b"\n").expect("write newline");
}

/// A persisted end offset resumes exactly at the next line, and a line is never
/// yielded twice nor skipped across a resume — the property that makes the
/// cursor safe to persist while the harness is still appending. Cannot prove
/// anything about a writer that rewrites earlier bytes; append-only is assumed.
#[test]
fn byte_offset_resume_reads_only_new_complete_lines() {
    let (_dir, path) = scratch();
    let first = write_records(&path, 4, 0);

    let head = read_all(&path, 0);
    assert_eq!(
        head.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>(),
        first
    );
    let mid = head[1].0;
    assert_eq!(
        read_all(&path, mid)
            .iter()
            .map(|(_, l)| l.clone())
            .collect::<Vec<_>>(),
        first[2..].to_vec(),
        "resuming from a mid-file cursor must yield exactly the unread lines"
    );

    let mut cursor = head[3].0;
    assert_eq!(cursor, size_of(&path));

    let appended: Vec<Vec<u8>> = vec![record(10, 0), record(11, 0)];
    let partial = {
        let mut p = record(13, 0);
        p.pop();
        p
    };
    let mut tail: Vec<u8> = appended.concat();
    tail.extend_from_slice(&partial);
    append(&path, &tail);

    let resumed = read_all(&path, cursor);
    assert_eq!(
        resumed.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>(),
        appended,
        "only the complete appended lines are readable"
    );
    cursor = resumed.last().expect("appended lines").0;
    assert_eq!(
        cursor,
        size_of(&path) - partial.len() as u64,
        "the cursor must stop before the partial line"
    );

    append(&path, b"\n");
    let final_lines = read_all(&path, cursor);
    assert_eq!(
        final_lines
            .iter()
            .map(|(_, l)| l.clone())
            .collect::<Vec<_>>(),
        vec![record(13, 0)],
        "the completed line is yielded exactly once, on the next pass"
    );
    assert_eq!(final_lines[0].0, size_of(&path));
    assert!(read_all(&path, final_lines[0].0).is_empty());
}

/// A partial trailing line is invisible to the reader and the reader latches
/// finished when it meets one, so a line completed by the harness *during* a
/// pass is left whole for the next pass instead of being stitched out of two
/// reads. Cannot prove anything about a harness that rewrites the partial bytes.
#[test]
fn partial_trailing_line_is_not_yielded_and_offset_stops_before_it() {
    let (_dir, path) = scratch();
    let lines = write_records(&path, 2, 0);
    let boundary = size_of(&path);
    append(&path, b"{\"seq\":2,\"kind\":\"fix");

    let mut reader = LineReader::open(&path, 0).expect("open");
    let mut got = Vec::new();
    while let Some(item) = reader.next_line() {
        got.push(item.expect("read"));
    }
    assert_eq!(got.len(), 2, "the partial line must not be yielded");
    assert_eq!(reader.offset(), boundary);

    // The harness completes the record while this reader is still alive.
    append(&path, b"ture\"}\n");
    assert!(
        reader.next_line().is_none(),
        "a finished reader must not resume into bytes that arrived after its EOF"
    );
    assert_eq!(reader.offset(), boundary, "the offset must not move");

    let next_pass = read_all(&path, boundary);
    assert_eq!(next_pass.len(), 1);
    assert_eq!(next_pass[0].0, size_of(&path));
    assert_eq!(
        got.into_iter()
            .map(|l| match l {
                Line::Complete { bytes, .. } => bytes,
                other => panic!("unexpected {other:?}"),
            })
            .collect::<Vec<_>>(),
        lines
    );
}

/// Two reads of an unchanged file yield identical `(offset, line)` sequences and
/// those offsets equal an independently computed running sum of line lengths, so
/// a cursor persisted by one pass means the same thing to the next. Cannot prove
/// determinism across a file that changed between the two reads.
#[test]
fn iter_lines_is_deterministic() {
    let (_dir, path) = scratch();
    let lines = write_records(&path, 500, 7);
    let first = read_all(&path, 0);
    let second = read_all(&path, 0);
    assert_eq!(first, second);
    assert_eq!(
        first.iter().map(|(o, _)| *o).collect::<Vec<_>>(),
        end_offsets(&lines)
    );
    assert_eq!(
        first.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>(),
        lines
    );
}

/// A line past the reader's limit is drained rather than assembled: the reader
/// reports its length so the caller can count one parse error, advances the
/// offset past it so it is skipped exactly once instead of blocking every future
/// read, keeps reading the lines after it, and holds no memory proportional to
/// it — the assembly buffer's capacity goes to zero when the limit trips and
/// never grows again, so an oversized line costs O(1), not O(line).
///
/// The limit is shrunk to 512 KiB so the fixture stays cheap; the last assertion
/// pins that `open` itself uses the real `MAX_LINE_BYTES`. Memory is asserted on
/// the accumulator rather than on peak RSS deliberately: `VmHWM` is a
/// process-wide high-water mark, so inside a shared test binary an 8 MiB buffer
/// under an already-higher peak measures as zero growth — an RSS assertion here
/// passes a buffer-it-whole implementation and therefore proves nothing. What
/// this cannot prove is the behaviour of the real 64 MiB limit end to end; the
/// `#[ignore]`d memory test streams a 200 MiB file for the file-size half.
#[test]
fn oversized_line_is_counted_and_skipped() {
    const LIMIT: u64 = 512 * 1024;
    let (_dir, path) = scratch();
    let head = record(0, 0);
    let tail = record(2, 0);
    let over_body = 8 * MIB - 1; // + newline = 16x the limit
    {
        let mut fh = BufWriter::new(File::create(&path).expect("create"));
        fh.write_all(&head).expect("write head");
        write_long_line(&mut fh, over_body);
        fh.write_all(&tail).expect("write tail");
        fh.flush().expect("flush");
    }

    let mut reader = LineReader::open_with_limit(&path, 0, LIMIT).expect("open");
    let mut got = Vec::new();
    while let Some(item) = reader.next_line() {
        got.push(item.expect("read"));
    }

    assert_eq!(
        got.len(),
        3,
        "the oversized line is reported, not swallowed"
    );
    assert_eq!(
        got[0],
        Line::Complete {
            end_offset: head.len() as u64,
            bytes: head.clone(),
        }
    );
    assert_eq!(
        got[1],
        Line::Oversized {
            end_offset: head.len() as u64 + over_body + 1,
            byte_len: over_body + 1,
        },
        "the caller learns the length and nothing else about the line"
    );
    assert_eq!(
        got[2],
        Line::Complete {
            end_offset: size_of(&path),
            bytes: tail,
        },
        "reading continues at the line boundary after the drained line"
    );
    assert_eq!(reader.offset(), size_of(&path));

    // The reader's only accumulator is `absorb`'s buffer, and every chunk of
    // every line goes through it: drive it with the same 8 MiB under the same
    // limit and the buffer must end owning nothing.
    let (mut buf, mut len, mut oversized) = (Vec::new(), 0u64, false);
    let chunk = vec![b'x'; READ_BUFFER_BYTES];
    let mut high_water = 0usize;
    while len < over_body {
        absorb(&mut buf, &mut len, &mut oversized, &chunk, LIMIT);
        high_water = high_water.max(buf.capacity());
    }
    assert!(oversized, "8 MiB under a 512 KiB limit is oversized");
    assert_eq!(
        buf.capacity(),
        0,
        "the buffer is handed back to the allocator, not kept for the rest of the line"
    );
    assert!(
        (high_water as u64) < LIMIT + READ_BUFFER_BYTES as u64 * 2,
        "the buffer peaked at {high_water} bytes, past the {LIMIT} byte limit"
    );

    assert_eq!(
        LineReader::open(&path, 0).expect("open").max_line_bytes,
        MAX_LINE_BYTES,
        "the product reader must use the real limit, not a test one"
    );
}

/// A cursor is trusted only while it still describes the file it names: a
/// mismatched path, an offset past the current end (truncation or replacement),
/// or a changed inode all force a read from byte 0, because resuming at a stale
/// offset would silently start mid-record in a different file's bytes.
#[test]
fn resume_offset_rejects_a_cursor_that_no_longer_describes_the_file() {
    let (_dir, path) = scratch();
    write_records(&path, 3, 0);
    let meta = std::fs::metadata(&path).expect("stat");
    let inode = inode_of(&meta);
    let valid = Cursor {
        path: path.clone(),
        byte_offset: 10,
        inode,
    };

    assert_eq!(
        resume_offset(None, &path, &meta),
        0,
        "no cursor: read it all"
    );
    assert_eq!(resume_offset(Some(&valid), &path, &meta), 10);
    assert_eq!(
        resume_offset(
            Some(&Cursor {
                path: path.with_extension("other"),
                ..valid.clone()
            }),
            &path,
            &meta
        ),
        0,
        "a cursor for another path says nothing about this one"
    );
    assert_eq!(
        resume_offset(
            Some(&Cursor {
                byte_offset: meta.len() + 1,
                ..valid.clone()
            }),
            &path,
            &meta
        ),
        0,
        "an offset past the end means the file shrank or was replaced"
    );
    assert_eq!(
        resume_offset(
            Some(&Cursor {
                byte_offset: meta.len(),
                ..valid.clone()
            }),
            &path,
            &meta
        ),
        meta.len(),
        "an offset exactly at the end is a complete, valid read"
    );

    let foreign = Cursor {
        inode: Some(inode.unwrap_or(0).wrapping_add(1)),
        ..valid.clone()
    };
    if cfg!(unix) {
        assert_eq!(
            resume_offset(Some(&foreign), &path, &meta),
            0,
            "a different inode is a different file"
        );
    } else {
        assert_eq!(
            resume_offset(Some(&foreign), &path, &meta),
            10,
            "off Unix there is no inode to compare, so validity is size-only"
        );
        assert_eq!(inode, None, "inode_of reports nothing off Unix");
    }
}

/// On POSIX an open reader keeps its inode, so a harness renaming the session
/// file mid-read (archive or rotate) neither errors nor drops lines: every line
/// and every offset still matches the original file. Cannot prove Windows
/// share-mode behaviour, nor anything about a harness truncating in place.
#[cfg(unix)]
#[test]
fn rename_while_open_keeps_reading_and_offsets() {
    let (_dir, path) = scratch();
    let moved = path.with_extension("archived.ndjson");
    let lines = write_records(&path, 3000, 0);

    let mut reader = LineReader::open(&path, 0).expect("open");
    let mut got = Vec::new();
    while let Some(item) = reader.next_line() {
        got.push(item.expect("read"));
        if got.len() == 1000 {
            std::fs::rename(&path, &moved).expect("rename mid-read");
            assert!(
                !path.exists(),
                "the rename must have happened mid-iteration"
            );
        }
    }

    let offsets: Vec<u64> = got
        .iter()
        .map(|l| match l {
            Line::Complete { end_offset, .. } => *end_offset,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(offsets, end_offsets(&lines));
    assert_eq!(*offsets.last().expect("lines"), size_of(&moved));
}

/// Renaming a session log while the reader holds it open must succeed, and the
/// open handle must keep yielding the rest of the file: the harness has to stay
/// free to rotate its own transcript underneath us.
///
/// **This test cannot fail if the `share_mode` call is deleted** — `std` already
/// passes those three bits by default, so it pins the *observable* guarantee, not
/// the mechanism. It would catch a future `std` narrowing its default, which is
/// exactly the risk the explicit call exists to absorb. Do not read it as proof
/// that the `share_mode` line is load-bearing today.
#[cfg(windows)]
#[test]
fn rename_while_open_succeeds_with_share_mode() {
    let (_dir, path) = scratch();
    let moved = path.with_extension("archived.ndjson");
    let lines = write_records(&path, 500, 0);

    let mut reader = LineReader::open(&path, 0).expect("open");
    let mut got = Vec::new();
    while let Some(item) = reader.next_line() {
        got.push(item.expect("read"));
        if got.len() == 100 {
            std::fs::rename(&path, &moved).expect("rename must succeed with FILE_SHARE_DELETE");
        }
    }
    assert_eq!(got.len(), lines.len(), "every line survives the rename");
    assert_eq!(reader.offset(), size_of(&moved));
}

/// Streaming a 200 MiB file grows peak RSS by less than one `MAX_LINE_BYTES`
/// while reading every line to the final offset, so memory is bounded by line
/// length and not by file size. Linux-only (it reads `/proc/self/status`), and
/// `#[ignore]`d because it writes 200 MiB: run with `cargo test -- --ignored`.
/// Skipped loudly, never silently, when the scratch filesystem has under 1 GiB
/// free. Cannot prove the bound for a single line longer than `MAX_LINE_BYTES`;
/// `oversized_line_is_counted_and_skipped` covers that case.
#[test]
#[ignore = "writes a 200 MiB fixture; run with --ignored"]
fn bounded_memory_large_file() {
    if !cfg!(target_os = "linux") {
        eprintln!("SKIP bounded_memory_large_file: peak RSS is read from /proc/self/status");
        return;
    }
    let (_dir, path) = scratch();
    match free_bytes(_dir.path()) {
        Some(free) if free < 1024 * MIB => {
            eprintln!(
                "SKIP bounded_memory_large_file: {} MiB free, under the 1 GiB the fixture needs",
                free / MIB
            );
            return;
        }
        None => {
            eprintln!("SKIP bounded_memory_large_file: could not measure free space");
            return;
        }
        _ => {}
    }

    let line = record(0, 160);
    let per_mib = MIB / line.len() as u64;
    let expected = per_mib * 200;
    {
        let mut fh = BufWriter::new(File::create(&path).expect("create"));
        for _ in 0..expected {
            fh.write_all(&line).expect("write");
        }
        fh.flush().expect("flush");
    }
    let size = size_of(&path);

    let before = peak_rss_bytes().expect("VmHWM");
    let mut reader = LineReader::open(&path, 0).expect("open");
    let mut n = 0u64;
    while let Some(item) = reader.next_line() {
        item.expect("read");
        n += 1;
    }
    let delta = peak_rss_bytes().expect("VmHWM").saturating_sub(before);

    assert_eq!(n, expected);
    assert_eq!(reader.offset(), size);
    assert!(
        delta < MAX_LINE_BYTES,
        "peak RSS grew by {} MiB reading {} MiB",
        delta / MIB,
        size / MIB
    );
}

/// Free bytes on the filesystem holding `dir`, via `df -Pk`; `None` when it
/// cannot be measured (the caller then skips loudly rather than guessing).
#[cfg(unix)]
fn free_bytes(dir: &Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let avail: u64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(avail * 1024)
}

#[cfg(not(unix))]
fn free_bytes(_dir: &Path) -> Option<u64> {
    None
}
