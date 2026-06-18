# Cisco Source Analysis Plan

This document describes the next architecture tranche for absorbing additional
Cisco DefenseClaw heuristics into vettd after Sprint 1.

Sprint 1 is already merged and covered:

- Apache 2.0 attribution scaffolding
- shared regex-based content pattern engine
- structured secret imports
- SSRF and cognitive tampering prompt/content imports
- declarative regex support for custom TOML rules

The remaining DefenseClaw material does not fit vettd's current model as a
direct port. DefenseClaw is largely finding-first and line-oriented, while
vettd is artifact-first and contract-oriented. The architecture below keeps
vettd aligned with its existing pipeline.

## Design constraints

- vettd should stay artifact-first, not become a per-line SAST engine
- new source scanning should emit stable aggregated artifacts and signals
- initial scope should stay bounded to `workdir` and `file` mode, or another
  explicitly limited surface
- contextual heuristics should live outside the generic content-pattern engine
- any directly adapted Apache-derived pattern families must update
  `THIRD_PARTY_NOTICES`

## Recommended shape

### New detector

Add a new native detector, tentatively `source_risks`, beside the existing
detectors in `crates/vettd-cli/src/detectors/`.

This detector should:

- scan bounded source and config files
- keep line-level matches internal only
- aggregate internal findings into one stable external artifact type
- emit signals that flow through the existing scoring, formatting, and
  verification pipeline

### Internal versus external model

Internally, the detector can use a lightweight line-oriented structure such as:

```rust
struct SourceFinding {
    family: &'static str,
    signal: String,
    path: PathBuf,
    line: Option<usize>,
    summary: String,
}
```

Externally, vettd should continue to expose `ArtifactReport` values rather than
per-line findings. The first proposed external artifact is:

- `source_risk_surface`

Suggested metadata:

- `paths`
- `top_risky_files`
- `matched_families`
- `finding_counts`
- `scanned_source_file_count`
- `scanned_json_file_count`

Suggested signal families:

- `source:dynamic_import`
- `source:nonliteral_require`
- `source:nonliteral_spawn`
- `source:ssrf_private_ip`
- `source:ssrf_internal_host`
- `source:sensitive_path_access`
- `source:cognitive_file_target`
- `json_config:secret`
- `json_config:metadata_url`
- `json_config:c2_url`

## Module layout

The cleanest fit for vettd is a small set of focused modules:

### `source_patterns.rs`

Compiled regex tables and constant definitions for source/config heuristics.
This should remain pure and have no I/O.

Use it for:

- simple source regexes
- JSON config patterns
- shared contextual helper regexes

Avoid putting multi-step heuristics here.

### `source_analysis.rs`

Pure helper logic that consumes file content and returns internal
`SourceFinding` values.

Use it for:

- network-context private IP detection
- internal-hostname plus fetch/request context detection
- dynamic import and non-literal require checks
- non-literal process execution checks
- sensitive-path and cognitive-file targeting heuristics

### `detectors/source_risks.rs`

Native detector implementation that:

- selects bounded candidate files
- reads supported source/config content
- calls the analysis helpers
- aggregates findings into a single `ArtifactReport` or another intentionally
  constrained artifact surface

## Scope boundaries

The first implementation should not scan everything everywhere.

Recommended initial boundaries:

- run in `workdir` and `file` mode only
- scan only known source/config suffixes
- keep byte limits similar to other bounded readers
- optionally bias toward files colocated with already-detected AI artifacts if
  scan cost needs tighter control

That gives vettd a usable source-analysis layer without turning `home` or
`host` mode into a generic code scanner.

## Execution order

The work was split into mergeable GitHub issues:

1. [#41](https://github.com/AgenticHighway/vettd-cli/issues/41) Add a bounded source-risk detector with an aggregated artifact model
2. [#42](https://github.com/AgenticHighway/vettd-cli/issues/42) Add JSON config scanning for secrets and suspicious destination URLs
3. [#44](https://github.com/AgenticHighway/vettd-cli/issues/44) Add contextual source heuristics for dynamic imports, process execution, and network-context SSRF targets
4. [#45](https://github.com/AgenticHighway/vettd-cli/issues/45) Add sensitive-path and cognitive-file targeting heuristics to source analysis
5. [#43](https://github.com/AgenticHighway/vettd-cli/issues/43) Decide scope for deferred DefenseClaw families: PII, vuln, malware, and broader exfiltration rules

Parent tracker:

- [#40](https://github.com/AgenticHighway/vettd-cli/issues/40) Phase 2: absorb remaining DefenseClaw source and config heuristics into vettd

## Proposed PR sequence

Use one narrow PR per child issue unless the remaining source-analysis slices
are intentionally finished together.

Recommended titles:

1. `feat: add bounded source-risk detector scaffold`
2. `feat: scan JSON configs for secrets and risky destinations`
3. `feat: add contextual source heuristics for dynamic execution and SSRF`
4. `feat: detect sensitive path access and cognitive file targeting`
5. `docs: record scope decision for deferred DefenseClaw families`

Final scope decision:

- see [docs/defenseclaw-scope-decision.md](defenseclaw-scope-decision.md) for the explicit list of approved versus out-of-scope DefenseClaw families.

## Explicit non-goals for the first slice

The first slice should not try to do all of the following at once:

- full PII classification
- broad vulnerability scanning
- malware signature scanning
- unrestricted per-line code indexing across the filesystem
- contract changes that force consumers to understand raw line findings

Those can be approved later if they fit vettd's product direction, but they
should not be smuggled into the initial source-analysis detector work.
