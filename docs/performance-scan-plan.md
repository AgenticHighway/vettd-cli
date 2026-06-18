# Scan Performance And Tiered Discovery Plan

This document defines the next architecture tranche for making `vettd` fast,
efficient, and accurate without turning normal usage into an antivirus-style
full-system crawl.

Related tracker:

- [#53](https://github.com/AgenticHighway/vettd-cli/issues/53) Performance roadmap: tiered discovery and efficient local scanning

## Why this work exists

Local profiling on a release build showed that bounded project scans are
already reasonably fast, while machine-wide scans are expensive enough to feel
like the old monolithic scanner model we want to avoid.

Observed local timings on a development Mac:

- `file --json` warm: about 0.04s
- `folder --json` warm: about 0.125s
- `repo --json` and `repo --contract` warm: about 0.06s
- `quick` warm: about 0.46-0.48s
- `scan --summary`: about 16s
- `scan --json`: about 17s
- `full --summary`: about 113s

Observed memory footprint:

- default home scan: about 356 MB peak RSS
- full root scan: about 2.27 GB peak RSS

The dominant cost is broad candidate discovery and repeated whole-list
iteration, not terminal formatting or JSON serialization.

## Current local baseline

Use `./scripts/benchmark-scanner.sh` to record a fresh local baseline. Set
`VETTD_TIMINGS=1` to include discovery, per-detector, analysis, and total scan
timings on stderr while the benchmark script runs.

Representative baseline on Will's mac after issue #55:

- `file --json`: about 0.04s
- `folder --json`: about 0.06s
- `repo --json`: about 0.06s
- `quick --json`: about 0.50s
- `scan --summary`: about 8.0s with about 165 MB peak RSS
- `full --summary`: about 113s with about 2.27 GB peak RSS

Representative detector-routing improvement from issue #56 against `origin/main`
using direct `VETTD_TIMINGS=1` release runs on the same machine:

- `quick --json`: `custom_rules` about `269ms -> 52ms`, `containers` about `4ms -> 2ms`, `mcp_configs` about `2ms -> 1ms`
- `scan --summary`: `custom_rules` about `1976ms -> 436ms`, `containers` about `49ms -> 33ms`, `mcp_configs` about `15ms -> 13ms`
- sampled `scan_total` moved from about `13.0s` to about `6.2s` on the comparison run, with discovery cost still dominating remaining runtime variance

## Design constraints

- `vettd` should remain local-first and operator-friendly on developer machines
- fast/default modes should prioritize high-value agent surfaces before broad traversal
- full-root scanning should remain available, but clearly as a heavier forensic mode
- performance improvements should not weaken detector correctness or contract stability
- scan architecture should prefer bounded work, reuse, and incremental behavior over brute-force breadth

## Proposed scan tier model

### Tier 0: Explicit target

User-selected file or folder. Always scan it.

### Tier 1: Critical OS-aware agent surfaces

High-signal locations where agent prompts, MCP configs, AI tool settings, and
local operator control files are likely to live.

Concrete roots in code:

- macOS: `~/Library/Application Support/Code/User`, `~/Library/Application Support/Cursor/User`, `~/.claude`, `~/.cursor`, `~/.aider`, `~/.continue`
- Linux: `~/.config/Code/User`, `~/.config/Cursor/User`, `~/.claude`, `~/.cursor`, `~/.aider`, `~/.continue`
- Windows: `%APPDATA%\\Code\\User`, `%APPDATA%\\Cursor\\User`, `%USERPROFILE%\\.claude`, `%USERPROFILE%\\.cursor`, `%USERPROFILE%\\.aider`, `%USERPROFILE%\\.continue`

### Tier 2: Adjacent project surfaces

Nearby source, container, and config files in repos or workspaces already shown
to contain agentic artifacts.

### Tier 3: Broad user-space scan

Bounded scans of selected user-space and project roots under the home directory, still exclusion-aware.

Concrete roots in code:

- macOS: bounded scans of `~/Desktop`, `~/Documents`, `~/Downloads`, `~/Developer`, `~/Projects`, `~/Code`, `~/Workspace`, `~/Work`, `~/src`, and `~/GitHub`, plus direct files in `~`
- Linux: bounded scans of `~/Desktop`, `~/Documents`, `~/Downloads`, `~/projects`, `~/code`, `~/workspace`, `~/work`, `~/src`, `~/git`, and `~/GitHub`, plus direct files in `~`
- Windows: bounded scans of `%USERPROFILE%\\Desktop`, `%USERPROFILE%\\Documents`, `%USERPROFILE%\\Downloads`, `%USERPROFILE%\\Projects`, `%USERPROFILE%\\Code`, `%USERPROFILE%\\Workspace`, `%USERPROFILE%\\Source`, `%USERPROFILE%\\src`, and `%USERPROFILE%\\GitHub`, plus direct files in `%USERPROFILE%`

### Tier 4: Forensic sweep

Explicit full-root scan. Expensive by design and not the normal recommendation.

## CLI mapping

Current mode semantics in code after issue #54:

- `quick`: Tier 1 only
- `scan`: Tier 1 + Tier 2 + bounded Tier 3
- `folder`: Tier 0 explicit target plus Tier 2-style local adjacency
- `repo`: Tier 0 explicit target plus deeper Tier 2 project adjacency
- `full`: Tier 4 forensic sweep only

## Engineering targets

### Discovery pruning

Non-forensic scans should skip low-value directories by default:

- build outputs (`target`, `dist`, `build`, `.next`)
- vendored dependency trees (`node_modules`, registry caches, virtualenvs)
- tool caches and temporary directories
- archive-like or machine-generated locations that rarely contain first-party agent control files

### Candidate routing

Avoid building one giant `Vec<Candidate>` and then asking every detector to walk
the same list. Prefer streaming or early prefiltering by basename, suffix, and
scan tier.

### Duplicate I/O removal

Matched file-backed artifacts should not reread full file contents when the same
bytes or digest were already computed during primitive gathering.

### Performance visibility

Add lightweight timing instrumentation around discovery and detector phases, plus
a repeatable local benchmark path for key scan modes.

### Incremental direction

Longer-term, normal scans should reuse cached metadata and eventually integrate
OS-native file change feeds so unchanged files do not get rescanned every run.

The first implementation slices are now in code for `quick`, default `scan`,
explicit `file`, and repeated explicit `folder` / `repo` scans:

- repeated runs persist scan profiles, file states, and detector output in
  `~/.vettd/scan-cache/scan-v1.sqlite3`
- unchanged file-backed detector inputs reuse cached artifacts for
  `custom_rules`, `containers`, and `mcp_configs`
- explicit `file` scans now reuse the same cache path, and file-mode
  `source_risks` participates because it is a deterministic single-file result
- on macOS, repeated `quick` and `scan` runs now also persist root replay
  cursors so unchanged bounded roots can skip a fresh directory walk entirely
- explicit `folder` and `repo` scans now reuse the same cache model, keyed by
  the resolved target root plus depth mode
- browser footprint detection remains uncached in this slice
- watcher-backed refresh outside macOS is still future work

The concrete design for that path now lives in
[incremental-scan-design.md](incremental-scan-design.md). The short version:

- start with a persistent stat-and-artifact cache for `quick` and `scan`
- treat OS-native change feeds as accelerators, not the only source of truth
- prefer replayable event history where the platform supports it
- keep `full` mode intentionally non-incremental by default

## Issue breakdown

The work is split into mergeable GitHub issues:

1. [#54](https://github.com/AgenticHighway/vettd-cli/issues/54) Define tiered OS-aware scan surfaces and align scan modes
2. [#55](https://github.com/AgenticHighway/vettd-cli/issues/55) Prune low-value discovery paths in non-forensic scans
3. [#56](https://github.com/AgenticHighway/vettd-cli/issues/56) Reduce candidate fan-out and memory pressure in the scan pipeline
4. [#57](https://github.com/AgenticHighway/vettd-cli/issues/57) Eliminate duplicate file reads and hashing in detection
5. [#58](https://github.com/AgenticHighway/vettd-cli/issues/58) Add scan benchmarking and performance guardrails
6. [#59](https://github.com/AgenticHighway/vettd-cli/issues/59) Design incremental scan caching and OS-native change tracking

Parent tracker:

- [#53](https://github.com/AgenticHighway/vettd-cli/issues/53) Performance roadmap: tiered discovery and efficient local scanning

## Proposed PR sequence

Use one narrow PR per child issue unless two adjacent slices are intentionally
finished together.

Recommended titles:

1. `docs: define tiered scan surfaces and mode mapping`
2. `perf: prune low-value paths in non-forensic discovery`
3. `perf: reduce candidate fan-out across detectors`
4. `perf: reuse file digests during detection`
5. `perf: add scan timing baselines and instrumentation`
6. `docs: design incremental scan cache and change tracking`

## Non-goals for the first performance slice

The first implementation should not try to do all of the following at once:

- rewrite every detector around a brand-new async runtime
- add kernel-level filesystem hooks to every platform in one PR
- weaken scan accuracy to improve headline timing
- make `full` mode pretend to be cheap when it is intentionally exhaustive

The early goal is to make the common local path feel efficient and deliberate.
The later goal is to make repeat scanning incremental rather than repetitive.
