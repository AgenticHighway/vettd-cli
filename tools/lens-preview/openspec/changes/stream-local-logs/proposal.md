# Proposal

## Why

On the owner's logs (2026-11-27), `--source local --json` peaked at about
1.84 GB of working set, although it prints 0.8 MB. `--json --history` added
only about 12 MB, so the cost came from the existing readers, not History
(recorded in `add-session-history` 4.4).

Measured cause: one recent Claude log file is 652 MB (the next largest is
33 MB; 287 recent Claude files total 1.4 GB). The shared line reader read
each file into memory whole, then parsed every line into a
`serde_json::Value` and held them all before the reader looked at any.
For that one file, the raw bytes and the parsed lines together made up
almost all of the peak:

| Step (release, owner's logs, `--source local --json`) | Peak     |
|--------------------------------------------------------|----------|
| Before                                                 | 1,841 MB |
| Read through a buffer, but every parsed line still held | 1,197 MB |
| Claude reader streams one line at a time               | 128 MB   |

## What Changes

- Log files are read through a buffer, one line at a time, instead of
  whole.
- The Claude reader handles each parsed line and drops it before reading
  the next. Codex rollouts (largest 44 MB) and the Codex session index are
  still collected per file, because the reader looks for `session_meta`
  before it walks the lines. They now skip the whole-file byte copy.
- Line rules are the same as before: "\n" or "\r\n" endings, blank lines
  skipped, invalid UTF-8 replaced, a bad or cut-off line skipped and the
  session marked partial, an unreadable file marked partial with no lines.

Output doesn't change. The one difference is a file that fails partway
through being read (an I/O error after it opened). Before, it gave no lines
at all. Now it keeps the lines read before the error. Either way the
session is marked partial.

## Capabilities

### Modified Capabilities
- `local-source`: log files are read a line at a time, so memory no longer
  grows with the size of the largest log file.

## Impact

- `src/local/mod.rs`: `lines_of` (a streaming line iterator) and
  `read_lines` built on it.
- `src/local/claude.rs`: iterates `lines_of` instead of a collected `Vec`.
- `src/local/tests.rs`: one test of the line rules.
- `docs/screen-test-results.md`: before and after numbers.
- No new dependencies. Plain `--json` and the fixture exports are
  byte-identical.
