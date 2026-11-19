# Proposal

## Why

Every Lens screen so far has been designed and checked only against a
12-session clean fixture. Real local logs hold thousands of sessions with long
names and thin copy identity. Slices 1–4 need a noisy, reproducible corpus to
test layout, timing and low-coverage states against. We also need a written
record of what the real aggregates measure and what they don't.

## What Changes

- Add a deterministic, seeded **stress corpus** of about 2,000 sessions over the
  same 7 days. It has 6 workspaces, long model IDs and paths, many small
  day-to-day changes, low copy coverage, duplicate session names, and some
  unpriced or incomplete runs.
- Add a `--corpus default|stress` option. `default` stays the current fixture,
  and an unknown value is rejected with a message on stderr and a non-zero exit.
- `--json` exports whichever corpus is selected. Repeated runs give
  byte-identical output. The default corpus export is unchanged apart from one
  added `corpus` field.
- The interactive preview starts on the selected corpus. Slice 0 only promises
  that it loads. Making the screens read well at stress scale belongs to
  Slices 1–2.
- Add `docs/coverage-note.md`, a one-page note. It records the real aggregate
  figures, names what they don't measure (copy coverage), and maps each stress
  parameter to a real figure or a labelled assumption.

## Capabilities

### New Capabilities
- `synthetic-corpus`: the selectable synthetic datasets that the preview and
  its JSON export are built from, and the guarantees they make
  (determinism, shape, load time, honest unknowns).

### Modified Capabilities
None. No specs exist yet.

## Impact

- `src/fixture.rs`: adds the stress generator. The current fixture stays as it
  is.
- `src/main.rs`: argument parsing, export of either corpus, and passing the
  selected corpus to the TUI.
- Other modules change only where they need the selected corpus passed in
  instead of calling the default fixture directly.
- No new dependencies. The random generator is a small inline function.
- Offline only. No real logs are read, and no raw local records go into the
  corpus or the note.
