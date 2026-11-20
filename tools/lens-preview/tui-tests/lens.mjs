// Shared launcher for the black-box journeys. tui-test 0.1.x has no runner or
// config file, so the settings a config would hold live here and the tests
// run under node:test.
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { TuiTest } from "@microsoft/tui-test";

const here = dirname(fileURLToPath(import.meta.url));
const exe = process.platform === "win32" ? "vettd-lens-preview.exe" : "vettd-lens-preview";
const targetDir = process.env.CARGO_TARGET_DIR
  ? resolve(process.env.CARGO_TARGET_DIR)
  : join(here, "..", "target");

export const BIN = process.env.LENS_BIN ?? join(targetDir, "debug", exe);
export const SIZE = { cols: 80, rows: 24 };
const TIMEOUTS = { text: 10_000, idle: 10_000, exit: 15_000, ready: 15_000 };

if (!existsSync(BIN)) {
  throw new Error(
    `${BIN} not found. Build it first: cargo build --locked --manifest-path tools/lens-preview/Cargo.toml`,
  );
}

/** Start the real binary in a pseudo-terminal at 80x24 and wait for Home. */
export async function launch(args = [], { waitFor = "WORTH A LOOK" } = {}) {
  const term = TuiTest.ephemeral("lens", { timeouts: TIMEOUTS });
  await term.run(BIN, args, SIZE);
  if (waitFor) await term.getByText(waitFor).expect();
  return term;
}

/** The program's exit code. `getExitCode()` only tracks shell commands, so
 * a program started with `run()` reports its code in `state().exited`. */
export async function exitCode(term) {
  await term.waitExit();
  return (await term.state()).exited;
}

/** Poll the screen until `check(text)` holds, the way a person waits for a
 * redraw, and fail with the last screen if it never does. */
export async function until(term, check, what, timeout = TIMEOUTS.text) {
  const end = Date.now() + timeout;
  let text = "";
  while (Date.now() < end) {
    text = await term.text();
    if (check(text)) return text;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`timed out waiting for ${what}:\n${text}`);
}

/** The selected row from its marker on, as `User::selected` reads it in
 * src/e2e/mod.rs. Home draws panels side by side, so text left of the
 * marker can belong to another panel. */
export function selected(text) {
  const line = text.split("\n").find((l) => l.includes("▸"));
  return line ? line.slice(line.indexOf("▸")).trimEnd() : null;
}

/** The breadcrumb row, as `User::crumb` reads it. */
export function crumb(text) {
  return (text.split("\n")[1] ?? "").trim();
}

export const selectedStartsWith = (term, prefix) =>
  until(term, (t) => selected(t)?.startsWith(prefix), `selection "${prefix}"`);
