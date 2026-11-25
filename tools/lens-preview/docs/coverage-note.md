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
| Sessions | 1,991 over Nov 12–18, every day present (276–293/day) | Assumption. Same order as the real Claude total (3,096), but the real period is unknown |
| Agent split | about 50/50 Claude/Codex | Assumption. Does **not** match real (3,096 vs 24) |
| Workspaces | 6, longest 94 chars | Assumption |
| Model IDs | 3 plus "Unpriced", longest 63 chars | Assumption |
| Session names | 8 names, each repeated 221–267 times | Assumption |
| Copy coverage | 128 of 1,391 skill uses (9.2%) record an artifact and version; the rest have neither | Assumption, labelled. Real share unmeasured |
| Unpriced requests | 63 of 4,980 (1.3%), with no rate (never a zero rate) | Assumption. Real: 0 unpriced models. Kept above real so the state is exercised |
| Incomplete sessions | 21 of 1,991 (1.1%); 19 of them have every request priced | Assumption. Real incompleteness is not measured; 0 skipped lines and 0 unreadable files is a different measure |
| Token part size | ≤ 1,000,000 (max 999,671) | Assumption. Bound keeps u32 cost maths from overflowing. The corpus token total (~3.37B) is summed in u64 |
| Components and artifacts | Existing 10 components and existing `PLACEMENTS` identities only. Uses go to the 9 callable components; the qa-pack plugin manifest's use is recorded on its children. Each use's one signal accounts for all its calls | Matches the default fixture, not real |
| Plan windows, limit hits, weekly ends, token sources | Same values as the default export | Reused default synthetic values. No session records them |

## Timings

Windows host, Rust 1.88.0 (Docker was unavailable for this run), release
build, ten runs of `--corpus stress --json` (1,298,620 bytes of output),
wall time in ms, including process start:

335, 347, 365, 356, 364, 363, 366, 386, 370, 367. Worst run: **386 ms**
(bound: 1 s). The earlier Docker `rust:1.97.1` run, before uses stopped
going to the plugin manifest, had a worst run of 50 ms.

Repeat-run sha256 (release):
- default: `33a1743f…2c60c2ef4` (both flag orders, and with `--corpus`
  omitted). It changed when the local-source and relay work merged in.
- stress: `c8ea73d4…853762f68` (both flag orders)

These measure synthetic generation and serialisation, not log parsing, so
they don't compare with the 195.7 s real Claude scan.
