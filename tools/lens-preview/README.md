# Lens interaction preview

This Rust development binary exercises the proposed Lens interaction with a
deterministic synthetic corpus. Run the prebuilt Windows executable or build
it from this directory with `cargo run --locked`. Pass `--json` to export the
same corpus. Add `--corpus stress` (anywhere on the command line) to load a
seeded ~2,000-session stress corpus instead of the default 12-session fixture;
any other `--corpus` value exits with code 2. The preview runs offline with no
log discovery or per-run writes.

The preview renders with [ratatui](https://ratatui.rs) on its crossterm
backend, drawing on stderr so stdout stays free for machine output. It is
its own Cargo workspace, so a broken preview build never affects vettd-cli.
It opens on Home, a dashboard: daily spend with your normal level, where
tokens went by source (an illustrative split, since the fixture doesn't
record sources), and a short Worth a look list whose rows open the older
views below. Render tests use ratatui's `TestBackend` at 80×24, 60×24 and
under `NO_COLOR`.

Plan headroom opens from Home's first finding. It lists each provider
window (Claude and Codex, 5-hour and weekly) with its used share, a
straight-line pace to reset, the reset time and its source: Codex logs, or
the opt-in Claude statusline relay. A window with one reading shows "—"
rather than a guessed pace. Window readings are synthetic fixture values.

CI runs the preview's checks in its own workflow (`lens-preview.yml`), only
when this folder changes. To run them locally in Docker from this folder:

```sh
docker run --rm -v "$PWD:/w" -w /w -e CARGO_TARGET_DIR=/tmp/target rust:1.88 \
  sh -c "rustup component add clippy rustfmt && cargo fmt --check && \
  cargo clippy --locked --all-targets -- -D warnings && cargo test --locked"
```

- Developer. Select Nov 16 and open the contributing sessions. Index rebuild
  (workspace api) accounts for $8.20 of the day's $8.52. Its components
  observed open at Nov 16 / api / Claude scope. Press p to widen that
  component to the week; Esc restores the day scope and selection.
- Maintainer. Press i. Narrow with w (workspace), a (agent), Tab (type) and
  e (evidence); x clears. The header shows the scope, result count and
  snapshot time. Open a component, then an exact copy, to see its locator,
  artifact, scan, resolution, competing copies and observations of the
  same artifact. Stage disable previews the predicted outcome; Apply is
  in memory only.

See "Connected slice" below for the corpus and identity rules.

The direct filter follows the keyboard conventions of [K9s](https://k9scli.io/topics/commands/).
Grouping the subagent under its parent follows the trace-tree pattern in
[LangSmith](https://docs.langchain.com/langsmith/cost-tracking). Rates, calls,
context attribution and grade evidence are illustrative fixture values.
The fixture never treats partial skill signals as definitive usage totals.
The chart toggle previews USD and request counts. Upstream count breakdowns
by trigger, tool, class and MCP remain Phase 1 parity work.

## Five phases

1. Local Lens in Vettd. Preserve the existing TUI capabilities, ingest
   canonical Claude and Codex records, run offline without per-run writes,
   and measure historical identity coverage early.
2. Historical reports. Explain dated changes by session, project and model.
   Compare equivalent rows with pinned prices, time zone and reference
   release. Reconcile the source lock at ccusage 20.0.49 with the roadmap
   target of 20.0.51 before running parity checks.
3. Live blocks and statusline. Separate freshness, provider limits, local
   estimates and coverage. Release watch independently of statusline
   latency work. See the [Claude statusline inputs](https://code.claude.com/docs/en/statusline)
   and [ccusage block reports](https://ccusage.com/guide/blocks-reports).
4. Cost and trust. Require version-matched scan evidence before displaying a
   skill grade. Retain unmatched spend, represent MCP verification separately
   and evaluate task effectiveness with task outcomes.
5. Additional adapters and optional directory contributions. Release adapters
   independently of a sharing service. Keep raw data local. Require separate
   opt-in and exact payload review for any directory contribution.

D1–D6 remain pending. Validate both walkthroughs with a person before
expanding the preview or choosing the production command and crate layout.

## Interaction design

| Pattern | How Lens uses it |
|---|---|
| [btop resource meters](https://github.com/aristocratos/btop) | A compact weekly chart pairs exact costs with a common scale. Gaps and partial input stay visible. |
| [htop tree view](https://github.com/htop-dev/htop/blob/main/htop.1.in) | A session keeps its main model and subagent together and opens into per-worker token counts. |
| [Yazi preview](https://yazi-rs.github.io/docs/quick-start/) | Moving the selection shows the day's cost change, the session's components, or the entity's usage before opening it. |
| [Lazygit incremental filtering](https://github.com/jesseduffield/lazygit/blob/master/docs/Searching.md) and [K9s key conventions](https://k9scli.io/topics/commands/) | Slash opens lookup from any view. Editing the query filters immediately. Enter opens the highlighted row, Esc restores the prior view, and ? shows keys. |

The preview pane folds away in a narrow terminal. The row marker, numeric
chart labels and Enter hint still identify focus and the pending action.
`NO_COLOR` uses reverse video for selection. All views derive their figures
from the same synthetic corpus that the JSON export returns.

For a human walkthrough, start at Activity with no instructions. See whether
the developer can predict what Enter will open from the chart and whether
the maintainer can find a known skill without choosing a date. Ask each
person to go Back after opening evidence and describe the scope they land in.

## Component inventory

Press i from Activity to browse installed skills, native tools and MCP
servers. Slash lookup opens the same component detail. From review-kit,
compare the workspace and user copies, then inspect observed versions and
signals. The synthetic 1.5 workspace copy has no matching scan. The 1.4
user copy has a fixture match. Historical records retain unknown versions.

Observed versions open into trigger or method counts and contributing
sessions. Each observation retains its historical version, signal names,
and estimated context cost. MCP method names are shown where recorded.
The exec wrapper shows wrapper calls and unknown inner commands. The
installed release-notes fixture has no observed use in the period.

Press Tab in Component inventory to filter by type. Open the workspace
copy of review-kit and select Stage disable. The preview names the target,
agent, and workspace, and shows the change from workspace 1.5 to user 1.4.
Esc cancels. Enter applies the simulation for the current run. Stage
re-enable restores the fixture override. This workflow uses a fixture
precedence rule and in-memory state. The JSON export contains the starting
fixture inventory and historical signals.
 
The request toggle changes the day chart, comparison, and session order.
Unpriced sessions show token evidence alongside an explicit pricing state.
Each named method or trigger opens matching observations. Real adapter
resolution, persistent config changes, and production identity remain pending.


## Connected slice

This section supersedes the component-inventory walkthrough above.

- Corpus. Workspaces api and web. review-kit 1.5 appears in both workspaces
  under different artifact identities (sha:7b20 and sha:e913). Only the api
  artifact has a fixture scan. Under Claude, the api workspace copy overrides
  the user 1.4 copy and falls back to it when disabled. The web Codex copy
  has unknown resolution. The corpus also includes historical observations
  with unknown versions, a partial Nov 18 session, an unverified MCP
  connection, and the qa-pack plugin with two linked children. It also
  includes a documented Claude Code hook and an unrecognized `tools.yaml`.
- Identity. Scan evidence and observation-to-copy links match by artifact
  only. A shared display name or version label never confers either.
  Historical observations are read-only and are never change targets.
- Scope. Each result shows workspace, agent and period. The scope travels
  with the view, so Esc restores the earlier scope, selection and scroll.
- Changes. Previews name the exact copy, locator, context, current and
  predicted resolution, and the basis for the prediction. Unsupported
  resolution previews an unknown outcome. Apply and re-enable affect only
  process-local state. The JSON export holds the starting fixture only.
