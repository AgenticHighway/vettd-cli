# Lens preview screen test

A moderated test of the preview with people, run before Slice 1. Until now
every judgment about the screens has been expert review. This test is the
first user evidence. The README asks for both walkthroughs to be "validated
with a person". This is that check.

## What it answers

1. Can a developer get from Home to the cause of an expensive day, and back
   again, without being told which keys to press?
2. Can a maintainer find a known skill and say which copy runs, without
   having to pick a date first?
3. After Esc, does each person know what scope they are back in?
4. Can people read Home at stress scale: unknown values, partial days, big
   token figures?

## Setup

- People: 3–5. At least one who uses Claude Code or Codex every day
  (developer role), and at least one who installs or reviews skills for
  others (maintainer role). One person can do both roles.
- Time: about 30 minutes each. The moderator reads the tasks aloud and
  doesn't demonstrate anything.
- Terminal: 80×24 or larger, colour on. Run from the repo root:

  ```powershell
  cargo build --locked --manifest-path tools/lens-preview/Cargo.toml
  ```

  ```powershell
  cargo run --locked --manifest-path tools/lens-preview/Cargo.toml
  ```

  For part C, restart the preview with the stress corpus:

  ```powershell
  cargo run --locked --manifest-path tools/lens-preview/Cargo.toml -- --corpus stress
  ```

- Say only: "This is a preview of a tool that shows what your coding agents
  used and what's installed. The data is made up. Think aloud. You can't
  break anything." Don't mention `?`, Enter, Esc or any other key. If someone
  is stuck for 60 seconds, say "What would you try next?" once. If they're
  still stuck, record an assist and move to the next task.

## Tasks

Read each task exactly as written. The expected answers come from the
default corpus and are checked by the render and end-to-end tests.

### A. Developer (default corpus)

| # | Task, as read aloud | Success looks like |
|---|---|---|
| A1 | "Before touching anything: what does this screen tell you about your week?" | Mentions spend or sessions (12 sessions, $12.36 priced) and at least one Worth a look row |
| A2 | "One day cost a lot more than usual. Find out which session caused it." | Reaches Nov 16 and names Index rebuild (about $8.20 of the day's $8.52) |
| A3 | "How many tokens did that session's main model take as fresh input?" | Opens the session's token detail and reads the Fresh input row |
| A4 | "Go back to where you started." | Returns to Home with the Nov 16 row still selected; says it's the same place |
| A5 | "Is anything about to run out?" | Opens the Claude 5-hour finding; says 62% used and full by about 13:22 at this pace |
| A6 | "Show the same week as request counts instead of dollars." | Finds `c`, unprompted or through `?` |

### B. Maintainer (default corpus)

| # | Task, as read aloud | Success looks like |
|---|---|---|
| B1 | "Find the skill called review-kit." | Uses `/` or `i` without picking a date first |
| B2 | "There's more than one copy of it. Which one does Claude use in the api workspace, and has it been scanned?" | Names the api workspace copy 1.5 (sha:7b20) and finds its scan |
| B3 | "Show only the MCP servers, then show everything again." | Filters by type, then clears it (back to 10 of 10) |
| B4 | "What would happen if you disabled the api copy? Don't make it permanent." | Opens Stage disable; says Claude falls back to the user copy 1.4; cancels or applies, knowing it lasts only for this run |
| B5 | "Go back. Where are you now, and what are you looking at?" | Describes the earlier scope and selection correctly |

### C. Stress corpus (either role, 5 minutes)

| # | Task, as read aloud | Success looks like |
|---|---|---|
| C1 | "This is a much bigger week. What's your normal daily spend?" | Says it's unknown because no day is complete. Doesn't read a number as normal |
| C2 | "Which source used the most tokens, and roughly how many?" | File reads, about 1.3B. Doesn't read it as smaller than Skills |
| C3 | "Are any of these daily totals exact?" | Says no, each is a lower bound ("every day partial") |

## What to record

Use one sheet per person, labelled P1, P2 and so on. Record no names or
employers. Keep the raw notes out of the repo.

| Task | Result (pass / assist / fail) | Time | Keys tried before success | Words they used for what they saw | Wrong prediction or misread |
|---|---|---|---|---|---|

Also note, at any point:
- every time someone predicted what Enter would open and was wrong;
- every number they misread, or read as more exact than it is;
- every time they couldn't say what scope they were in after Esc.

## Gate decision

Once every session is done, write `docs/screen-test-results.md` with
aggregates only: pass, assist and fail counts per task, plus the misreads
that came up more than once, paraphrased. Then the person who owns the gate
decides:

- **Pass:** no task fails for more than one person, and C1–C3 have no
  misread numbers. Slice 1 starts as planned.
- **Pass with changes:** a task fails or needs an assist for two or more
  people. Turn each of those into a Slice 1 requirement before Slice 1
  starts.
- **Fail:** A2, A4, B2 or B5 fails for most people. Redesign that flow
  before any Slice 1 work.

These thresholds are a proposal. The gate owner can change them before the
first session, not after.

A waiver instead of this test should be recorded in the same results file,
with the reason, so later slices know their design has no user evidence.
