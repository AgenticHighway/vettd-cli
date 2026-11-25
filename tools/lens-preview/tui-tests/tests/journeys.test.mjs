// Black-box journeys: the real binary in a real pseudo-terminal, driven by
// keystrokes. Expectations mirror src/tests/render.rs, src/tests/corpus.rs
// and src/e2e/mod.rs, which assert the same strings through App::press.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { BIN, crumb, exitCode, launch, selected, selectedStartsWith, until } from "../lens.mjs";

const T = { timeout: 60_000 };

async function journey(args, body) {
  const term = await launch(args);
  try {
    await body(term);
  } finally {
    await term.closeQuiet();
  }
}

// The first screen must answer "what did I use and where did it go" in the
// terminal people actually run, not just in a TestBackend buffer.
test("1. default Home shows usage, sources, findings and the plan window", T, () =>
  journey([], async (term) => {
    for (const text of ["12 sessions", "$12.36 priced", "DAILY SPEND", "WHERE TOKENS WENT", "WORTH A LOOK"]) {
      await term.getByText(text).expect();
    }
    await selectedStartsWith(term, "▸ Claude 5-hour: 62%, full by 13:22");
  }));

// "Why did spend jump?" must reach the day's sessions and Esc must return
// to the same finding, or people lose their place after every drill-down.
test("2. Down, Enter opens Nov 16; Esc returns with Nov 16 selected", T, () =>
  journey([], async (term) => {
    await term.press("Down");
    await selectedStartsWith(term, "▸ Nov 16: 10× a normal day");
    await term.press("Enter");
    await term.getByText("Nov 16 sessions").expect();
    await selectedStartsWith(term, "▸ Index rebuild");
    await term.press("Escape");
    await until(term, (t) => crumb(t) === "Home", "Home breadcrumb");
    await selectedStartsWith(term, "▸ Nov 16");
  }));

// The unit toggle changes what Home measures; a real key must flip it both
// ways or the chart silently stays in the wrong unit.
test("3. c toggles DAILY SPEND and DAILY REQUESTS", T, () =>
  journey([], async (term) => {
    await term.press("c");
    await term.getByText("DAILY REQUESTS").expect();
    await term.getByText("DAILY SPEND").expect({ not: true });
    await term.press("c");
    await term.getByText("DAILY SPEND").expect();
  }));

// Filters must be undoable from the keyboard: Tab narrows, x restores the
// full inventory, so nobody is stranded on a filtered list.
test("4. i opens inventory; Tab narrows; x restores 10 of 10", T, () =>
  journey([], async (term) => {
    await term.press("i");
    await until(term, (t) => crumb(t) === "Home › Components", "Components breadcrumb");
    await term.getByText("10 of 10").expect();
    await term.press("Tab");
    await term.getByText("10 of 10").expect({ not: true });
    await term.press("x");
    await term.getByText("10 of 10").expect();
  }));

// Lookup is a text field: typed letters must search, not fire shortcuts,
// even through a real terminal's key encoding.
test("5. / then typing rev selects review-kit", T, () =>
  journey([], async (term) => {
    await term.press("/");
    await term.type("rev");
    await until(term, (t) => selected(t)?.includes("review-kit"), "review-kit selected");
  }));

// Quitting must restore the terminal and report success, so scripts and
// shells that run the preview don't see a failure.
test("6. q exits with code 0", T, async () => {
  const term = await launch();
  try {
    await term.press("q");
    assert.equal(await exitCode(term), 0);
  } finally {
    await term.closeQuiet();
  }
});

// At stress scale no day is complete and figures run to billions and
// thousands of dollars; the real terminal must show the honest wording and
// whole numbers, never $0.00 or a label cut mid-number.
test("7. stress Home shows unknown normal, billions and whole-dollar labels", T, () =>
  journey(["--corpus", "stress"], async (term) => {
    for (const text of ["normal: no complete day", "every day partial (≥)", "3.4B tokens", "File reads", "1.3B", "12 $1885 "]) {
      await term.getByText(text).expect();
    }
    const text = await term.text();
    assert.ok(!text.includes("$0.00"), text);
    for (let day = 12; day <= 18; day++) {
      const start = `${day} $`;
      const at = text.indexOf(start);
      assert.ok(at >= 0, `day ${day} label missing:\n${text}`);
      // tui-test trims row ends, so the last label ends at a newline.
      const amount = text.slice(at + start.length).split(/\s/)[0];
      assert.match(amount, /^\d+$/, `day ${day} label cut to $${amount}:\n${text}`);
    }
  }));

// A bad flag must fail loudly with a usage code and a message a person can
// read, not open a TUI on the wrong data.
test("8. --corpus other exits 2 with the error on stderr", T, async () => {
  const term = await launch(["--corpus", "other"], { waitFor: null });
  try {
    await term.getByText("vettd-lens-preview: --corpus takes default or stress").expect();
    assert.equal(await exitCode(term), 2);
  } finally {
    await term.closeQuiet();
  }
  // A PTY merges stdout and stderr, so check the stream outside it.
  const out = spawnSync(BIN, ["--corpus", "other"], { encoding: "utf8" });
  assert.equal(out.status, 2);
  assert.equal(out.stdout, "");
  assert.equal(out.stderr.trim(), "vettd-lens-preview: --corpus takes default or stress");
});
