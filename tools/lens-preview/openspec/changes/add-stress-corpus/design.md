# Design

## Context

See proposal.md (Why) and specs/synthetic-corpus/spec.md for the requirements.
Re-grounded on the tracked preview at PR #332's head (`aa37924`). The code
works like this today:

- `fixture::sessions()` (`src/fixture/sessions.rs`) builds 12 sessions.
  `main()` (`src/main.rs`) calls it once and passes `&[Session]` to `run`,
  which hands it to `App::frame`, `App::draw` and `App::press`. Tests in
  `src/tests/` and `src/e2e/` call `fixture::sessions()` directly.
- Arguments are read with `std::env::args().nth(1).as_deref() == Some("--json")`.
- `Session`, `Request` and `Use` (`src/fixture/sessions.rs`) and `Placement`
  and `Window` (`src/fixture/mod.rs`) hold `&'static str` fields.
  `Use.component` indexes the 10-entry `COMPONENTS` array, and any value of
  10 or more panics.
- `Request::cents` multiplies `tokens * rate` in u32. That overflows above
  about 71M tokens per part at the highest fixture rate (60). The aggregates
  built on it (`Session::cents`, the corpus total in `src/plan.rs`, the day
  totals in `src/views.rs`) are u32 sums. `model::bar` widens to usize.
- The week is hardcoded to days 12–18 in `Scope::WEEK` (`src/model.rs`) and in
  the Week view's `(12..=18)` loop (`src/views.rs`). Sessions outside that
  range are silently missing from the Week view.
- `WORKSPACES` (`["api","web"]`, `src/fixture/mod.rs`) has two users:
  - the `--json` export, which would misreport the stress corpus;
  - the inventory workspace filter (`w`) in `src/app.rs`. That filter cycles
    over `PLACEMENTS`, which only use `api` and `web`, so it is not
    corpus-dependent.
- Besides sessions, the export carries corpus-independent fixture data:
  `period`, `snapshot`, `agents`, `components`, `placements`, `scans`, and the
  plan-headroom and Home data: `windows`, `now_minutes_from_nov_18`,
  `claude_weekly_ends_percent`, `limit_hits` and `token_sources` (basis points
  summing to 10,000, an illustrative split the sessions don't record).
  `plan::filling()` reads the static `WINDOWS`, not the sessions.
- serde_json is built without `preserve_order` (no `indexmap` in
  `Cargo.lock`), so `json!` objects serialize with sorted keys, which is
  deterministic.
- Long strings in fixed-width row formats are clipped by ratatui when drawn.
  No panic is expected; task 3.4 checks it.

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
2. **Stress strings come from `const` pools of `&'static str` literals**
   (names, workspaces, models). No string is generated at runtime, so nothing
   is leaked.
   - This keeps every existing type and signature.
   - Superseded during apply: an earlier draft leaked generated strings with
     `Box::leak`. The fixed pools made that unnecessary.
   - Rejected: changing the fields to `String` or `Cow`. That touches every
     screen module for no behavior gain in this slice.
3. **The stress generator lives in a new `src/fixture/stress.rs`**, re-exported
   from `src/fixture/mod.rs` like `sessions.rs`. `sessions()` is untouched.
4. **The stress generator stays inside the fixed week, days 12–18.** That
   avoids touching `Scope::WEEK` and the Week loop, which belong to later
   slices. Every day has sessions, so this corpus has no day-13 gap.
5. **Bounded magnitudes.**
   - Each token part stays at or below 1,000,000, and rates come from the same
     small set the default fixture uses (max 60). Each part then costs at most
     6,000 cents, far below the u32 limit in `Request::cents`.
   - At most 4 requests per session keeps the whole-corpus u32 sums under
     about 2,100 × 4 × 24,000 ≈ 202M cents, also far below the limit.
6. **Copy coverage is about 10%, as a labelled assumption.**
   - Skill uses (components 0, 4, 6) record an existing artifact identity from
     `PLACEMENTS` for that component, with its version, in about 10% of cases.
   - The rest have `artifact: None` and `version: None`.
   - The real share is unmeasured, and the coverage note says so.
7. **Unknowns.**
   - About 1% of requests use an `Unpriced` model with `milli_usd_per_ktok:
     None`.
   - About 1% of sessions are `incomplete: true`.
8. **Duplicate names and long identifiers.**
   - Session names come from a small pool, so each name repeats many times.
   - Workspaces are 6 names, at least one longer than 80 characters.
   - Model IDs come from a pool that includes IDs over 40 characters.
9. **Arguments.**
   - Scan all of the arguments: `--json` anywhere, and `--corpus <name>`
     anywhere, replacing the `args().nth(1)` check.
   - A missing or unknown name writes `error: --corpus must be default or
     stress` to stderr, writes nothing to stdout and exits with code 2.
   - Other arguments are still ignored, as they are today.
   - The TUI path passes the selected corpus to `run`; `App` is unchanged.
10. **Export.**
    - `export` takes the corpus name as well as the sessions, and adds
      `"corpus": "<name>"`.
    - `workspaces` becomes the sorted, distinct workspaces of the selected
      corpus. For the default corpus that is `["api","web"]`, the same as
      before. The inventory filter keeps using the `WORKSPACES` constant.
    - Everything else stays as it is.
11. **Plan-window and source fields are reused, not derived.**
    - The stress export emits the same `windows`, `now_minutes_from_nov_18`,
      `claude_weekly_ends_percent`, `limit_hits` and `token_sources` as the
      default export.
    - Why: in both corpora these are synthetic provider readings and an
      illustrative split. No session records sources or limit readings, so
      deriving them from stress sessions would invent data. Reusing them keeps
      Home and Plan headroom populated with explicit values; no zero stands in
      for "unknown".
    - The coverage note labels them "reused default synthetic values".
    - Rejected: deriving from stress sessions (nothing to derive from), and
      emitting zeros or omitting the fields (misreads as measured or breaks
      the export shape).
12. **The default export is pinned by a golden file.**
    - Capture the `--json` output of the unchanged branch head (`aa37924`,
      which already includes the plan and source fields) to
      `tests/golden/default-export.json` before changing any code.
    - A unit test parses the golden file and the new export, removes `corpus`
      from the new one, and asserts the two are equal as `serde_json::Value`.

## Overlap with add-lens-tui-dashboard

The sibling change `add-lens-tui-dashboard` (not yet on the PR branch; it
lives untracked on local branch `claude/lens-tui-dashboard-spec`) specs the
same `--json` export under a `json-export` capability. This change specs the
export behavior it adds (`corpus`, `workspaces` from the selected corpus,
reused plan fields) under `synthetic-corpus`. Whichever change is archived
second must reconcile the export scenarios of the two capabilities so the
main specs don't contradict each other. This change does not edit the
sibling.

## Risks / Trade-offs

- [The stress corpus looks too regular, so Slices 1–2 miss real noise]
  → The coverage note lists each parameter as "matches real" or "assumption".
  Revisit it when copy-coverage aggregates exist.
- [Reused plan windows don't match stress session volume]
  → Accepted for Slice 0: both corpora's windows are synthetic. Labelled in
  the coverage note.
- [The timing check is environment-dependent]
  → Measure with a release build in Docker over ten runs and record the worst
  run. The debug test only guards against regressing to slow generation, with
  the same 1 s bound on corpus construction.

## Open Questions

- The real copy-coverage share. It needs a new aggregate from real logs, and
  Slice 3's low-coverage threshold depends on it. It doesn't change this
  slice's tasks.
