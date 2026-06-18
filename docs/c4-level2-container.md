# C4 Level 2 — Container Diagram

Shows the major runtime containers and data stores within the **vettd** system boundary.

```mermaid
flowchart TD
    subgraph VettdCLI["vettd CLI Binary (Rust)"]
        CLI["CLI Layer\n(clap parsing, command dispatch,\nwizard, auth, setup, update)"]
        ScanEngine["Scan Engine\n(discovery, detection,\nrisk scoring, verification)"]
        OutputLayer["Local Output Layer\nterminal rendering, JSON/file writes,\npost-scan next-step prompt"]
        Submission["Submission Pipeline\ncontract build, auth resolution,\ncontract sync, HTTP retry"]
        Updater["Self-Updater\nmanifest + signature verify,\ndownload, SHA-256 verify,\nbackup + replace"]
        RuleEngine["Rule Engine\n(loads and validates\ncustom TOML rules)"]
    end

    subgraph LocalStorage["Local Filesystem"]
        AuthConfig["~/.config/vettd/config.json\n(API key + endpoint)"]
        ScannerUUID["~/.vettd/scanner_uuid\n(persistent identity)"]
        RulesDir["~/.vettd/rules/*.toml\n(custom detection rules)"]
        ContractCache["~/.vettd/contract/\n(cached schema + version)"]
        ScanCache["~/.vettd/scan-cache/\n(SQLite scan profiles, file states,\ndetector artifacts for repeated scans,\nfuture root cursors)"]
        UpdateFiles["~/.vettd/downloads/\n~/.vettd/vettd.backup"]
        AccessFile[".vettd.toml\n(access mode + endpoint overrides)"]
        ReportFiles["vettd-report.json /\nvettd-contract.json /\ncustom output path"]
    end

    subgraph ExternalSystems["External Systems"]
        Backend["Compatible Backend\n(optional ingest + review UI)\ncurrent hosted example: vettd remote"]
        ReleaseAPI["Hosted Release Metadata API\nmanifest + signature endpoints"]
        ReleaseHost["GitHub Releases\nplatform archives"]
    end

    FS["Target Filesystem\n(scanned files and directories)"]

    CLI -->|"dispatches scan mode"| ScanEngine
    CLI -->|"manages rules"| RuleEngine
    CLI -->|"reads access settings"| AccessFile
    ScanEngine -->|"reads candidates"| FS
    ScanEngine -->|"loads rules"| RulesDir
    ScanEngine -.->|"reuses unchanged detector results"| ScanCache
    ScanEngine -->|"produces ScanReport"| OutputLayer
    OutputLayer -->|"writes local reports"| ReportFiles
    OutputLayer -->|"submits when requested"| Submission
    Submission -->|"reads credentials"| AuthConfig
    Submission -->|"checks contract version"| ContractCache
    Submission -->|"GET /api/contract + POST /api/scans/ingest"| Backend
    CLI -->|"persists credentials"| AuthConfig
    CLI -->|"reads/writes identity"| ScannerUUID
    CLI -->|"triggers"| Updater
    Updater -->|"uses downloads + backup"| UpdateFiles
    Updater -->|"GET latest + signature"| ReleaseAPI
    ReleaseAPI -.->|"currently served by"| Backend
    Updater -->|"downloads platform archive"| ReleaseHost
```

## Container Responsibilities

| Container           | Technology          | Purpose                                                                                       |
| ------------------- | ------------------- | --------------------------------------------------------------------------------------------- |
| CLI Layer           | clap + crossterm    | Parse commands, run wizard/setup/auth/update flows, apply access gating                       |
| Scan Engine         | walkdir + detectors | Discover filesystem candidates, run detectors, score risk, verify                             |
| Local Output Layer  | serde + ANSI output | Render terminal output, write JSON files, offer post-scan next steps                          |
| Submission Pipeline | ureq (HTTP)         | Build contract payloads, resolve auth, sync contract version, submit payload                  |
| Self-Updater        | ureq + flate2/tar   | Verify signed manifests, download platform archives, verify SHA-256, swap binary              |
| Rule Engine         | toml + validation   | Load, validate, install custom `.toml` detection rules                                        |
| Local Storage       | Filesystem          | Persist identity, auth, rules, access settings, contract cache, scan cache, update temp files |
