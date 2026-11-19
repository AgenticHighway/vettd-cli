# Design

## Context

See proposal.md (Why) and specs/synthetic-corpus/spec.md for the requirements.
The code works like this today:

- `fixture::sessions()` builds 12 sessions. `main()` calls it once and passes
  `&[Session]` to `run`, `content`, `draw` and `components::rows`. Nothing else
  calls it outside tests.
- Arguments are read with `args().nth(1) == Some("--json")`.
- `Session`, `Request` and `Use` hold `&'static str` fields. `Use.component`
  indexes the 10-entry `COMPONENTS` array, and any value of 10 or more panics.
- `Request::cents` multiplies `tokens * rate` in u32. That overflows above about
  70M tokens per part at the highest rate.
- The week is hardcoded to days 12–18 in `Scope::WEEK`, the Week view and the
  gaps count. Sessions outside that range are silently dropped.
- Long strings overflow the fixed-width row formats. Rows are truncated rather
  than panicking.
- `--json` exports the constant `WORKSPACES` (`["api","web"]`), which would
  misreport the stress corpus.
- serde_json is built without `preserve_order`, so `json!` objects serialize
  with sorted keys, which is deterministic.

## Goals / Non-Goals

**Goals:**
- A second corpus that exercises scale, long strings, duplicates and low
  coverage, selected at startup.
- Byte-stable JSON for both corpora, guarded by tests.

**Non-Goals:**
- Making the screens read well at stress scale: column widths, scrolling,
  answer-line widths. That belongs to Slices 1–2.
- New components or placements. Stress uses refer to the existing 10 components
  and the existing artifact identities.
- Reading real logs or `*.local.json` files.

## Decisions

1. **Seeded inline PRNG (splitmix64) with a fixed seed constant.**
   - This needs no new dependency, and the output is identical on every
     platform.
   - Rejected: the `rand` crate, which adds a dependency, and its output can
     drift between versions.
2. **Generated strings are leaked once to `&'static str`** with `Box::leak`,
   about 2,000 names at startup in a dev binary.
   - This keeps every existing type and signature.
   - Rejected: changing the fields to `String` or `Cow`. That touches every
     screen module for no behavior gain in this slice.
3. **The stress generator stays inside the fixed week, days 12–18.** That
   avoids touching the hardcoded week ranges, which belong to later slices.
   Every day has sessions, so this corpus has no day-13 gap.
4. **Bounded magnitudes.** Each token part stays at or below 1,000,000, and
   rates come from the same small set the default fixture uses. That keeps
   `Request::cents` well below u32 overflow.
5. **Copy coverage is about 10%, as a labelled assumption.**
   - Skill uses (components 0, 4, 6) record an existing artifact identity from
     `PLACEMENTS`, with its version, in about 10% of cases.
   - The rest have `artifact: None` and `version: None`.
   - The real share is unmeasured, and the coverage note says so.
6. **Unknowns.**
   - About 1% of requests use an `Unpriced` model with `milli_usd_per_ktok:
     None`.
   - About 1% of sessions are `incomplete: true`.
7. **Duplicate names and long identifiers.**
   - Session names come from a small pool, so each name repeats hundreds of
     times.
   - Workspaces are 6 names, at least one longer than 80 characters.
   - Model IDs come from a pool that includes IDs over 40 characters.
8. **Arguments.**
   - Scan all of the arguments: `--json` anywhere, and `--corpus <name>` anywhere.
   - A missing or unknown name writes `error: --corpus must be default or
     stress` to stderr and exits with code 2.
   - Other arguments are still ignored, as they are today.
9. **Export.**
   - Add `"corpus": "<name>"`.
   - `workspaces` becomes the sorted, distinct workspaces of the selected
     corpus. For the default corpus that is `["api","web"]`, the same as
     before.
   - Everything else stays as it is.
10. **The default export is pinned by a golden file.**
    - Capture today's `--json` output to `tests/golden/default-export.json`
      before changing any code.
    - A unit test parses the golden file and the new export, removes `corpus`
      from the new one, and asserts the two are equal as `serde_json::Value`.

## Risks / Trade-offs

- [The stress corpus looks too regular, so Slices 1–2 miss real noise]
  → The coverage note lists each parameter as "matches real" or "assumption".
  Revisit it when copy-coverage aggregates exist.
- [The leaked strings are never freed]
  → This happens once per process in a preview binary. The design notes it so
  nobody copies the pattern into production code.
- [The timing check is environment-dependent]
  → Measure with a release build in Docker over ten runs and record the worst
  run. The debug test only guards against regressing to slow generation, with
  the same 1 s bound on corpus construction.

## Open Questions

- The real copy-coverage share. It needs a new aggregate from real logs, and
  Slice 3's low-coverage threshold depends on it. It doesn't change this
  slice's tasks.
