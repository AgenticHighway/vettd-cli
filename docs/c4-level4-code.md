# C4 Level 4 — Code Diagrams

Zooms into key data flows and type relationships at the code level.

## Scan Pipeline Sequence

The core scan execution from CLI invocation through to output, including the
interactive post-scan next-step menu for local terminal runs.

```mermaid
sequenceDiagram
    participant User
    participant CLI as cli::run()
    participant Scan as scan::run_scan()
    participant Disc as discovery
    participant Det as detectors
    participant Risk as risk_engine
    participant Ver as verifier
    participant Out as output::emit()
    participant Wiz as wizard
    participant Con as contract::build_contract_payload()
    participant File as output::write_json_report()

    User->>CLI: vettd scan file agents.md
    CLI->>Scan: run_scan("file", Some(path), None, false)
    Scan->>Disc: discover_file_surface(path)
    Disc-->>Scan: Vec<Candidate>
    loop for each Detector
        Scan->>Det: detector.detect(candidates, deep)
        Det-->>Scan: Vec<ArtifactReport>
    end
    loop for each ArtifactReport
        Scan->>Risk: score_artifact(artifact)
        Risk-->>Scan: scored artifact
        Scan->>Ver: verify(artifact)
        Ver-->>Scan: verified artifact
    end
    Scan-->>CLI: ScanReport
    CLI->>Out: emit(report, ...)
    Out-->>User: Human / summary / JSON output
    alt interactive TTY and no --json/--contract/--submit
        CLI->>Wiz: pick("Next step")
        alt Write report to disk
            Wiz-->>CLI: SaveReport + path
            CLI->>File: write_json_report(report, duration_ms, path)
            File->>Con: build_contract_payload(report, duration_ms)
            Con-->>File: ContractPayload
            File-->>User: Report written to disk
        else Submit results to Vettd
            Wiz-->>CLI: SubmitToVettd
            CLI-->>User: Continue into submission flow
        else Do nothing
            Wiz-->>CLI: DoNothing
        end
    end
```

## Access Gate & Output Branching

How access settings, output flags, and TTY detection shape the user-visible
result after a scan completes.

```mermaid
flowchart TD
    Report["ScanReport returned from scan.rs"]
    Access["cli.rs\nload_access_config()"]
    Gate{"Access mode"}
    Lite["lite_mode.rs\nlimit_lite_mode_report()"]
    Full["Keep full artifact set"]
    Severity["filter_by_severity()"]
    Flags{"Output / submit flags?"}
    Human["formatters.rs\nprint overview / full / summary"]
    Json["output.rs\nprint JSON to stdout"]
    Contract["cli.rs + contract/\nbuild contract payload"]
    Save["write_json_report() /\nwrite contract file"]
    Submit["resolve auth → contract sync →\nsubmit_contract_payload()"]
    Prompt{"TTY and no\n--json / --contract / --submit?"}
    Next["wizard::pick(\"Next step\")"]
    Done["Exit"]

    Report --> Access --> Gate
    Gate -->|"lite"| Lite --> Severity
    Gate -->|"licensed / default"| Full --> Severity
    Severity --> Flags
    Flags -->|"default / --full / --summary"| Human --> Prompt
    Flags -->|"--json"| Json --> Done
    Flags -->|"--out"| Human
    Human -->|"optional --out"| Save
    Flags -->|"--contract"| Contract
    Contract -->|"optional --out"| Save
    Contract -->|"--submit"| Submit --> Done
    Prompt -->|"yes"| Next
    Prompt -->|"no"| Done
    Next -->|"Write report to disk"| Save --> Done
    Next -->|"Submit results"| Submit
    Next -->|"Do nothing"| Done
```

## Submission Flow Sequence

The submission path when `--submit` is used or the interactive post-scan prompt
continues into submission.

```mermaid
sequenceDiagram
    participant CLI as cli::run()
    participant Out as output::do_submit()
    participant Auth as saved config / flags
    participant Sync as contract_sync::sync_contract()
    participant Con as contract::build_contract_payload()
    participant HTTP as submit::submit_contract_payload()
    participant Backend as Compatible Backend

    CLI->>Con: build_contract_payload(report, duration)
    Con-->>CLI: ContractPayload JSON
    CLI->>Out: do_submit(payload_json, auth)
    Out->>Auth: resolve auth from flags or config
    Auth-->>Out: AuthConfig (key + endpoint)
    Out->>Sync: sync_contract(endpoint)
    Sync->>Backend: GET /api/contract?version=true
    Backend-->>Sync: version response
    alt compiled contract mismatch
        Sync-->>Out: explicit error + update guidance
        Out-->>CLI: submission blocked
    else compatible / unreachable / server error
        Sync-->>Out: continue
        Out->>HTTP: submit_contract_payload(payload, key, endpoint)
        loop up to 3 retries (5s / 30s / 120s)
            HTTP->>Backend: POST /api/scans/ingest (Bearer token)
            Backend-->>HTTP: 201 / 409 / 429 / 5xx
        end
        HTTP-->>Out: success / explicit error
    end
```

## Self-Update Flow Sequence

Binary update check, confirmation, download, and replacement.

```mermaid
sequenceDiagram
    participant User
    participant CLI as cli::run()
    participant Upd as updater
    participant Meta as Hosted Release Metadata API
    participant Host as GitHub Releases / artifact host

    User->>CLI: vettd update / vettd update --check
    CLI->>Upd: check_for_update()
    Upd->>Meta: GET latest manifest
    Upd->>Meta: GET latest signature
    Meta-->>Upd: manifest + signature envelope
    Upd->>Upd: verify ECDSA signature
    Upd->>Upd: compare semver + resolve platform artifact
    Upd-->>CLI: UpdateCheckResult (is_newer)
    alt --check only
        CLI-->>User: print update status
    else is_newer = true
        alt force = false
            CLI-->>User: Proceed with update? [Y/n]
            User-->>CLI: confirm / cancel
        end
        alt confirmed
            CLI->>Upd: perform_update(force)
            Upd->>Host: GET artifact URL
            Host-->>Upd: platform archive
            Upd->>Upd: SHA-256 verify
            Upd->>Upd: backup current binary
            Upd->>Upd: extract and replace
            Upd-->>CLI: success
        else cancelled
            CLI-->>User: update cancelled
        end
    else is_newer = false
        CLI-->>User: already up to date
    end
```

## Core Data Types

```mermaid
flowchart LR
    subgraph models.rs["models.rs — Core Types"]
        AR["ArtifactReport\n─────────────\nartifact_type: String\nconfidence: f64\nsignals: Vec of String\nmetadata: Map\nrisk_score: i32\nrisk_reasons: Vec of String\nverification_status: String\nartifact_id: String\nartifact_hash: String\nregistry_eligible: bool\nartifact_scope: String"]
        SR["ScanReport\n─────────────\nscanner_version: String\ntimestamp: String\nscanned_path: String\nartifacts: Vec of ArtifactReport\ntotal_artifacts: usize\nscan_mode: String"]
    end

    subgraph contract_types["contract/types.rs — Contract Types"]
        CP["ContractPayload\n─────────────\nscan_meta: ScanMeta\nprompts: Vec of Prompt\nskills: Vec of Skill\nmcp_servers: Vec of McpServer\nagents: Vec of Agent\nagentic_apps: Vec of AgenticApp"]
        SM["ScanMeta\n─────────────\nscan_id: String\nendpoint_hostname: String\nscanned_at: String\nscanner_version: String\nscan_duration_ms: u64\nscan_roots: Vec of String\nhost_network: HostNetworkInfo"]
    end

    SR -->|"contains many"| AR
    SR -->|"transformed into"| CP
    CP -->|"contains"| SM
```

## Detector Trait and Implementations

```mermaid
flowchart TD
    Trait["trait Detector\n─────────────\nname() -> &str\ndetect(candidates, deep)\n  -> Vec of ArtifactReport"]

    CRD["CustomRulesDetector\n(loads TOML rules from\n~/.vettd/rules/)"]
    CD["ContainerDetector\n(Dockerfile, docker-compose,\ncontainer_kind metadata)"]
    MCD["MCPConfigDetector\n(VS Code, Cursor, Claude\nMCP server configs)"]
    BFD["BrowserFootprintDetector\n(Chrome/Edge/Firefox\nextension artifacts)"]

    Trait -.->|"implemented by"| CRD
    Trait -.->|"implemented by"| CD
    Trait -.->|"implemented by"| MCD
    Trait -.->|"implemented by"| BFD

    RE["rule_engine.rs\n(DetectionRule, MatchConfig,\nKeywordConfig)"]
    CRD -->|"loads rules via"| RE
```

## Discovery Modes

```mermaid
flowchart TD
    RunScan["run_scan(mode)"]

    RunScan -->|"mode = host"| Host["discover_host_surfaces()\nBounded AI config dirs\n(Cursor, VS Code, Claude, etc.)"]
    RunScan -->|"mode = scan"| Home["discover_scan_surfaces()\nTier 1 critical roots +\nbounded user-space roots"]
    RunScan -->|"mode = root"| Root["discover_root_surfaces()\nEntire filesystem from /"]
    RunScan -->|"mode = workdir"| Workdir["discover_workdir_surfaces()\nExplicit project directory\n(deep mode optional)"]
    RunScan -->|"mode = file"| File["discover_file_surface()\nSingle file analysis"]
    RunScan -->|"mode = filesystem"| FS["discover_filesystem_surfaces()\nHome + system app paths"]

    Host -->|"Vec of Candidate"| Detectors["Detector Pipeline"]
    Home -->|"Vec of Candidate"| Detectors
    Root -->|"Vec of Candidate"| Detectors
    Workdir -->|"Vec of Candidate"| Detectors
    File -->|"Vec of Candidate"| Detectors
    FS -->|"Vec of Candidate"| Detectors
```
