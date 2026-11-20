# Stress corpus coverage note

What the stress corpus (`--corpus stress`) is based on, and what it isn't.
Aggregates only. No raw record, path or identifier from local logs appears
here.

## Real figures (source: local aggregate test report, 2026-11-17, not checked in)

| Agent  | Sessions | Size     | Skipped lines | Unreadable files | Unpriced models | Scan time |
|--------|---------:|----------|--------------:|-----------------:|----------------:|----------:|
| Claude | 3,096    | ~2.9 GB  | 0             | 0                | 0               | 195.7 s (debug build) |
| Codex  | 24       | not recorded | 0         | 0                | 0               | 4.61 s    |

The report does not state the period those sessions span, the workspace
count, name or identifier lengths, or how often a run is incomplete.

**Not measured: copy and identity coverage.** No aggregate records how often
a skill use names which copy (artifact) ran, or its version. This note makes
no estimate of it. Slice 3's low-coverage threshold waits on that figure.

## Stress parameters

Seed `0x5EED_1A75_2026_1118`, splitmix64. Figures are from the generated
corpus.

| Parameter | Stress value | Basis |
|-----------|--------------|-------|
| Sessions | 2,011 over Nov 12–18, every day present (272–297/day) | Assumption. Same order as the real Claude total (3,096), but the real period is unknown |
| Agent split | about 50/50 Claude/Codex | Assumption. Does **not** match real (3,096 vs 24) |
| Workspaces | 6, longest 94 chars | Assumption |
| Model IDs | 3 plus "Unpriced", longest 63 chars | Assumption |
| Session names | 8 names, each repeated 220–278 times | Assumption |
| Copy coverage | 111 of 1,178 skill uses (9.4%) record an artifact and version; the rest have neither | Assumption, labelled. Real share unmeasured |
| Unpriced requests | 47 of 4,952 (0.95%), with no rate (never a zero rate) | Assumption. Real: 0 unpriced models. Kept above real so the state is exercised |
| Incomplete sessions | 19 of 2,011 (0.94%) | Assumption. Real incompleteness is not measured; 0 skipped lines and 0 unreadable files is a different measure |
| Token part size | ≤ 1,000,000 (max 999,799) | Assumption. Bound keeps u32 cost maths from overflowing |
| Components and artifacts | Existing 10 components and existing `PLACEMENTS` identities only | Matches the default fixture, not real |
| Plan windows, limit hits, weekly ends, token sources | Same values as the default export | Reused default synthetic values. No session records them |

## Timings

Docker `rust:1.97.1`, release build, ten runs of `--corpus stress --json`
(1,296,985 bytes of output), wall time in ms:

46, 41, 50, 50, 46, 48, 43, 45, 44, 43. Worst run: **50 ms** (bound: 1 s).

Repeat-run sha256 (release):
- default: `2f641dd0…16cf5f` (both runs, and with `--corpus` omitted)
- stress: `d6b8d5c3…197469` (both runs, either flag order)

These measure synthetic generation and serialisation, not log parsing, so
they don't compare with the 195.7 s real Claude scan.
