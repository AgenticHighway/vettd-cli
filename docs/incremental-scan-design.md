# Incremental Scan Cache And Change Tracking Design

This document captures the design for issue `#59`.

Status:

- phase 1 stat-cache implementation now exists for `quick`, `scan`, `folder`,
  `repo`, and `file`
- macOS root-refresh now uses persisted FSEvents cursors for repeated `quick`
  and `scan` runs, falling back to bounded rewalks whenever replay is missing
  or untrusted
- watcher-backed refresh for other platforms remains future work
- this document still defines the broader roadmap beyond the shipped first
  slice

## Why this exists

`vettd` already improved broad discovery and detector fan-out, but the default
local path still pays discovery and detector cost on every run. That is fine
for correctness, but it leaves obvious reuse on the table for the common case:

- the same operator runs `quick` or `scan` repeatedly on the same machine
- most files under Tier 1 and Tier 3 roots have not changed
- detector logic and custom rules have not changed

The next performance step is therefore cross-run reuse, not just single-run
efficiency.

## Design goals

- keep `vettd` local-first and operator-friendly
- avoid weakening correctness in exchange for faster headline timings
- make `quick` and default `scan` the primary incremental beneficiaries
- preserve explicit-target semantics for `file`, `folder`, and `repo`
- keep `full` as an intentionally exhaustive mode rather than pretending it is
  cheap
- treat OS-native change signals as accelerators, not as the sole source of
  truth

## Non-goals

- no background daemon in the first implementation
- no kernel hook or long-lived watcher service in this issue
- no attempt to make `full` mode incremental by default
- no cache-dependent correctness where a missing cache entry changes results
- no remote or multi-machine cache synchronization

## Current state

Today `vettd` has:

- per-run file primitives such as `content_hash`, `file_size_bytes`, and
  `last_modified`
- bounded scan tiers for `quick` and `scan`
- discovery pruning for non-forensic walks
- detector routing and timing instrumentation

What it does not yet have:

- cross-platform OS change cursors or replay state for all scan roots

## Proposed cache scope by mode

### `quick`

Primary incremental target.

- cache reads: yes
- cache writes: yes
- OS change integration: macOS FSEvents replay now shipped for repeated runs;
  other platforms remain future work

### `scan`

Primary incremental target.

- cache reads: yes
- cache writes: yes
- shares Tier 1 reuse with `quick`
- tracks additional bounded user-space roots separately from `quick`
- repeated macOS runs now reuse cached root membership when FSEvents replay
  reports no changes for a bounded root

### `folder`

Explicit target with bounded adjacency.

- cache reads: yes
- cache writes: yes
- no long-lived watcher requirement in the initial rollout
- intended mainly as write-through reuse for repeated scans of the same folder

### `repo`

Explicit target with deeper adjacency.

- cache reads: yes
- cache writes: yes
- good fit for repeated local repo scans
- no watcher dependency in the initial rollout

### `file`

Explicit single target.

- cache reads: yes
- cache writes: yes
- `source_risks` now joins the cacheable set here because file mode produces a
  deterministic single-target surface instead of a workdir aggregate

### `full`

Explicit forensic sweep.

- cache reads: no by default
- cache writes: optional and deferred
- purposefully remains exhaustive and expensive

## Cache identity model

Incremental reuse needs more than a path string. The design should use four
layers of identity.

### 1. Scan profile fingerprint

Defines whether cached artifacts are even comparable to the current run.

Inputs:

- cache schema version
- `vettd` binary version
- scan mode (`quick`, `scan`, `folder`, `repo`)
- deep flag where relevant
- resolved scan roots
- detector-set fingerprint
- built-in rule fingerprint
- custom rule directory fingerprint
- discovery exclusion-set fingerprint

If the scan profile fingerprint changes, cached artifact reuse for that profile
is invalid.

### 2. File identity key

Defines whether a specific file is still the same filesystem object.

Stored fields:

- canonical path
- origin tier (`host`, `home`, `workdir`)
- stable file identifier where available
- file size in bytes
- modification time with nanosecond precision where available

Recommended stable file identifiers:

- macOS and Linux: device id + inode
- Windows: volume serial number + file index

Fallback when a stable file id is unavailable:

- canonical path + size + mtime

### 3. Content verification key

Defines whether content-sensitive reuse is safe when stat-level evidence is not
enough.

Stored field:

- cached SHA-256 content digest from the prior scan

Digest reuse rules:

- do not recompute the digest on every run just to decide reuse
- reuse the cached digest when the file identity key is unchanged
- recompute only when file identity changed, the filesystem has coarse mtime
  precision, or the platform fallback path is weak enough that extra certainty
  is required

### 4. Artifact bundle fingerprint

Defines whether the serialized detector output for a file can be reused.

Stored fields:

- artifact hash or serialized artifact payload
- detector name(s) that produced the artifact
- detector/rule fingerprint used for that artifact

This allows unchanged files to skip both discovery-time rereads and
detector-time analysis when neither the file nor the relevant detector profile
changed.

## Reuse predicate

A cached artifact bundle is reusable when all of the following are true:

1. the scan profile fingerprint matches
2. the file still exists inside the resolved scan surface
3. the file identity key matches the cached row
4. the relevant detector/rule fingerprint matches
5. no higher-priority invalidation reason exists for the root or profile

When any of those checks fail, the file is rescanned and the cache is replaced.

## Proposed on-disk layout

Recommended storage:

- `~/.vettd/scan-cache/scan-v1.sqlite3`

SQLite is the right default for the expected entry count because it supports:

- tens of thousands of rows without loading everything into memory
- transactional updates when scans are interrupted
- targeted invalidation by profile, root, or path
- compact persistence without managing many per-root JSON files

Recommended logical tables:

- `scan_profiles`
    - `profile_key`, mode, roots, detector fingerprint, rule fingerprint,
      completed_at
- `file_states`
    - canonical path, origin tier, stable file id, size, mtime, content hash,
      last_seen_profile
- `artifacts`
    - `file_state_id`, artifact type, serialized artifact payload, artifact hash,
      detector fingerprint
- `root_cursors`
    - root path, backend type, persisted cursor/token where the platform supports
      replayable change history

Important detail:

- file state should be global, not duplicated per mode, so Tier 1 files can be
  reused between `quick` and `scan`
- profile membership should be tracked separately, since `scan` includes a
  superset of the `quick` roots

## Interaction with scan tiers and explicit target scans

### Tier 1 and Tier 3

These are the best incremental candidates because they are stable, bounded, and
operator-driven.

- `quick` should reuse unchanged Tier 1 files aggressively
- `scan` should reuse Tier 1 files plus bounded user-space/project files

### Tier 2 adjacency

Tier 2 should remain derived from the target roots and local artifact presence.

- `folder` and `repo` can cache file states for repeated explicit scans
- cache keys must include the explicit target root so unrelated repos do not
  pollute each other

### Tier 0 explicit single-file scans

These remain correctness-first and simple.

- a cache lookup is acceptable but not required for the first implementation

### Tier 4 full-root scans

These should remain brute-force by default.

- they may write cache rows opportunistically later
- they should not rely on cache reads to claim forensic completeness

## OS-native change tracking evaluation

The core design constraint is that `vettd` is currently a short-lived CLI, not
an always-on daemon. That matters because some watcher APIs only report changes
while a process is actively running.

### macOS: FSEvents

Best fit for future cross-run acceleration.

Why:

- supports directory-tree level monitoring
- exposes replayable event IDs that can be persisted between runs
- matches the bounded-root model already used by `quick` and `scan`

Tradeoffs:

- reports directory/subtree changes, not fully resolved file payloads
- still requires targeted restat or subtree refresh after replayed events

Recommendation:

- preferred event backend for macOS incremental refresh
- persist per-root event IDs in `root_cursors`

### Linux: inotify

Good live watcher, poor cross-run replay source for a short-lived CLI.

Why:

- works well for unprivileged process-local watching
- integrates naturally with bounded roots

Tradeoffs:

- no replayable history across separate CLI invocations
- recursive watching requires registering many directories
- watch-count limits and queue overflows need explicit fallback handling

Recommendation:

- not sufficient by itself for cross-run incremental scans without a later
  long-lived helper
- acceptable only as a future live-session accelerator or daemon-backed option

### Linux: fanotify

Not a strong initial fit.

Why:

- mount-oriented rather than naturally bounded to user-selected roots
- more privilege-sensitive and operationally heavier than `inotify`

Recommendation:

- defer
- do not choose as the first Linux backend for `vettd`

### Windows: ReadDirectoryChangesW

Reasonable live watcher, but like `inotify`, it does not solve cross-run reuse
for a short-lived CLI on its own.

Why:

- supports subtree watching per root
- fits bounded Tier 1 and Tier 3 roots

Tradeoffs:

- no durable change history across invocations
- requires a running process or future helper service for continuity

Recommendation:

- useful only if `vettd` later accepts a background helper

### Windows: USN Journal

Best Windows candidate for cross-run replay, but more complex than the first
incremental rollout needs.

Why:

- provides durable volume-level change history
- better conceptual match for persisted cursors between CLI runs

Tradeoffs:

- more complex than root-scoped watchers
- volume-level semantics require additional filtering back down to `vettd`'s
  bounded roots

Recommendation:

- evaluate after stat-cache rollout if Windows incremental behavior becomes a
  high-priority target

## Recommended rollout phases

### Phase 1: Stat cache only

Add persistent file state and artifact reuse without any watcher integration.

- target `quick` and `scan` first
- use stat tuple + stable file id as the primary unchanged test
- recompute content hash only when stat evidence is insufficient

This phase provides cross-run wins on every platform without changing the CLI
process model.

### Phase 2: macOS replay-backed refresh

Add FSEvents-backed root cursors for `quick` and `scan` on macOS.

- replay changes since the last successful scan
- restat only touched subtrees instead of rewalking every configured root

### Phase 3: Optional explicit-target reuse

Extend the same cache to `folder` and `repo` for repeated local scans.

- no long-lived watcher required
- scoped by explicit target root

### Phase 4: Linux and Windows live backends if desired

Only if `vettd` accepts a longer-lived helper or a more platform-specific
incremental layer.

- Linux: `inotify` helper if a daemon model is approved
- Windows: `ReadDirectoryChangesW` helper or later USN Journal integration

## Invalidation and fallback rules

- detector, rule, or cache-schema changes invalidate artifact reuse
- queue overflow, event gap, or missing persisted cursor triggers a bounded
  root rescan
- root path changes or canonicalization changes invalidate the affected root
- unsupported or weak filesystems fall back to stat-cache or full rescans
- interruption during cache update should leave the previous committed state
  intact

## Recommended next implementation issue

The first implementation issue after this design should be narrowly scoped:

- introduce `~/.vettd/scan-cache/scan-v1.sqlite3`
- persist scan profiles, file states, and serialized artifact bundles
- enable unchanged-file reuse for `quick` and `scan` using stat keys first
- do not add watcher backends in the first implementation PR

That keeps the first incremental slice compatible with the current CLI model
while leaving room for macOS replay-backed acceleration later.
