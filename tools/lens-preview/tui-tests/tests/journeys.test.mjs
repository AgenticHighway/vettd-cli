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
    for (const text of ["normal: no complete day", "every day partial (≥)", "3.4B tokens", "File reads", "1.3B", "12 $1909 "]) {
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

// The counter at the end of the hint row ("n/total"), so a journey can tell
// where the selection is in a long list.
function counter(text) {
  const hint = text.split("\n").find((l) => l.includes(" Enter "));
  return hint?.trim().split(/\s+/).pop() ?? null;
}

// Reaching a day used to take seven presses through Week. In a real
// terminal the arrow keys must choose a bar and Enter open it, and Esc must
// come back with the same bar still chosen.
test("9. Right, Right, Enter opens Nov 13; Esc keeps the bar chosen", T, () =>
  journey([], async (term) => {
    await term.press("Right");
    await term.press("Right");
    await term.getByText("Enter See Nov 13 sessions").expect();
    await term.press("Enter");
    await until(term, (t) => crumb(t) === "Home › Nov 13", "Nov 13 breadcrumb");
    await term.getByText("No session input.").expect();
    await term.press("Escape");
    await until(term, (t) => crumb(t) === "Home" && t.includes("13▸"), "Home with Nov 13 chosen");
  }));

// A digit opens a day straight from Home, and Esc returns there.
test("10. 5 opens Nov 16 with Index rebuild first; Esc returns Home", T, () =>
  journey([], async (term) => {
    await term.press("5");
    await until(term, (t) => crumb(t) === "Home › Nov 16", "Nov 16 breadcrumb");
    await selectedStartsWith(term, "▸ Index rebuild");
    await term.press("Escape");
    await until(term, (t) => crumb(t) === "Home", "Home breadcrumb");
  }));

// Find reaches a session by name, and Esc keeps the search and the row,
// so a person can open the next result without typing again.
test("11. / index finds the Nov 16 run; Esc keeps search and row", T, () =>
  journey([], async (term) => {
    await term.press("/");
    await term.type("index");
    await term.getByText("── Sessions (6)").expect();
    for (let i = 0; i < 3; i++) await term.press("Down");
    await selectedStartsWith(term, "▸ Nov 16 Index rebuild");
    await term.press("Enter");
    await until(term, (t) => crumb(t).endsWith("Index rebuild") && t.includes("$8.20"), "Nov 16 session");
    await term.press("Escape");
    await term.getByText("Search: index_").expect();
    await selectedStartsWith(term, "▸ Nov 16 Index rebuild");
  }));

// A 289-row stress day (Nov 16): End and g must reach both ends in one press each.
test("12. stress Nov 16: End reaches the last session, g the first", T, () =>
  journey(["--corpus", "stress"], async (term) => {
    await term.press("5");
    // Home has a counter too, so wait for the day before reading it.
    await until(
      term,
      (t) => crumb(t) === "Home › Nov 16" && /^1\/\d+$/.test(counter(t) ?? ""),
      "Nov 16 first row counter",
    );
    const total = counter(await term.text()).split("/")[1];
    assert.ok(Number(total) > 100, `only ${total} sessions`);
    await term.press("End");
    await until(term, (t) => counter(t) === `${total}/${total}`, "last row counter");
    await term.press("g");
    await until(term, (t) => counter(t) === `1/${total}`, "back to the first row");
  }));

// The new keys must be findable without being told: the footer names the
// day arrows and help lists every new key.
test("13. footer names the day arrows; help lists the new keys", T, () =>
  journey([], async (term) => {
    await term.getByText("←→ Day").expect();
    await term.press("?");
    for (const text of ["Home/End First or last row", "Home: choose a day", "Home, Week: open that day"]) {
      await term.getByText(text).expect();
    }
  }));
