# C4 Level 3 — Component Diagram

Shows the internal Rust modules within the **vettd** crate and their relationships.

## Scan Engine Components

```mermaid
flowchart TD
    subgraph ScanOrchestration["Scan Orchestration"]
        scan["scan.rs\n(run_scan — orchestrates\ndiscovery → detect → score → verify)"]
    end

    subgraph Discovery["Discovery Layer"]
        discovery["discovery.rs\n(host, home, root, workdir,\nfile surface enumeration)"]
    end

    subgraph Detectors["Detection Layer"]
        detector_base["base.rs\n(Detector trait)"]
        custom_rules["custom_rules.rs\n(TOML rule-based detector)"]
        containers["containers.rs\n(Dockerfile, compose)"]
        mcp_configs["mcp_configs.rs\n(MCP server configs)"]
        browser["browser_footprints.rs\n(browser extension artifacts)"]
    end

    subgraph Analysis["Analysis Layer"]
        risk["risk_engine.rs\n(score_artifact — signal\nand type-based scoring)"]
        verifier["verifier.rs\n(verify — pass/conditional/fail\ngovernance classification)"]
        capabilities["capabilities.rs\n(derive_capabilities —\nkeyword-to-capability mapping)"]
    end

    subgraph RuleSystem["Rule System"]
        rule_engine["rule_engine.rs\n(load, parse, validate\nTOML detection rules)"]
        rules_cmd["rules.rs\n(add, remove, list CLI\nrule management)"]
    end

    scan -->|"1. enumerate candidates"| discovery
    scan -->|"2. run detectors"| detector_base
    detector_base -.->|"implemented by"| custom_rules
    detector_base -.->|"implemented by"| containers
    detector_base -.->|"implemented by"| mcp_configs
    detector_base -.->|"implemented by"| browser
    custom_rules -->|"loads rules"| rule_engine
    scan -->|"3. score each artifact"| risk
    scan -->|"4. verify each artifact"| verifier
    risk -->|"uses"| capabilities
```

## Contract & Output Components

```mermaid
flowchart TD
    subgraph ContractLayer["Contract Builder (contract/)"]
        contract_mod["mod.rs\n(build_contract_payload —\npartition and transform)"]
        prompts["prompts.rs\n(build_prompts)"]
        agents["agents.rs\n(build_agents)"]
        skills["skills.rs\n(build_skills)"]
        mcp["mcp.rs\n(build MCP server entries)"]
        apps["apps.rs\n(build_agentic_apps)"]
        types["types.rs\n(ContractPayload, ScanMeta,\nPrompt, Skill, McpServer, Agent, AgenticApp)"]
    end

    subgraph OutputLayer["Output Layer"]
        output["output.rs\n(emit, do_submit — local output\nand submission orchestration)"]
        write_json["output.rs\n(write_json_report —\npersist report JSON)"]
        formatters["formatters.rs\n(print_overview, print_human,\nprint_summary — ANSI terminal)"]
        lite_mode["lite_mode.rs\n(access-gated result limiting\nfor lite mode)"]
    end

    subgraph Models["Core Data Models"]
        models["models.rs\n(ArtifactReport, ScanReport —\ncanonical v1 schema types)"]
    end

    contract_mod -->|"delegates to"| prompts
    contract_mod -->|"delegates to"| agents
    contract_mod -->|"delegates to"| skills
    contract_mod -->|"delegates to"| mcp
    contract_mod -->|"delegates to"| apps
    contract_mod -->|"uses types from"| types
    output -->|"builds payload via"| contract_mod
    output -->|"writes report via"| write_json
    output -->|"human output via"| formatters
    output -->|"lite filtering via"| lite_mode
    write_json -->|"builds payload via"| contract_mod
    contract_mod -->|"reads"| models
    formatters -->|"reads"| models
```

## Side-Effect & Infrastructure Components

```mermaid
flowchart LR
    subgraph SideEffects["Side-Effect Modules"]
        submit["submit.rs\n(auth config persistence,\nHTTP POST with retry)"]
        network["network.rs\n(endpoint validation,\nprivate/local host checks)"]
        network_evidence["network_evidence.rs\n(firewall rules, MCP transport,\nenv var refs, log scanning)"]
        updater["updater.rs\n(signed manifest verify,\ndownload, SHA-256 verify,\nbinary swap)"]
        contract_sync["contract_sync.rs\n(fetch/cache contract schema,\nversion mismatch detection)"]
        identity["identity.rs\n(scanner_uuid persistence,\naccount_uuid resolution)"]
    end

    subgraph UserInteraction["User Interaction"]
        cli["cli.rs\n(Cli, Commands, OutputArgs —\ncommand dispatch, access gate,\npost-scan next-step prompt)"]
        wizard["wizard.rs\n(interactive scan mode\nselection and pick/ask prompts)"]
        setup["setup.rs\n(optional auth + endpoint\nsetup wizard)"]
        progress["progress.rs\n(terminal progress\nindicator)"]
    end

    cli -->|"interactive fallback + next-step menu"| wizard
    cli -->|"setup command"| setup
    cli -->|"submission dispatch"| submit
    submit -->|"validates endpoint"| network
    cli -->|"update commands"| updater
    cli -->|"contract sync"| contract_sync
    cli -->|"resolves identity"| identity
```

## Module Index

| Module                            | Layer            | Responsibility                                  |
| --------------------------------- | ---------------- | ----------------------------------------------- |
| `cli.rs`                          | User Interaction | CLI argument parsing, command dispatch, access gating |
| `wizard.rs`                       | User Interaction | Interactive scan mode picker                    |
| `setup.rs`                        | User Interaction | Optional auth and endpoint setup                |
| `progress.rs`                     | User Interaction | Terminal progress indicator                     |
| `discovery.rs`                    | Discovery        | Filesystem candidate enumeration                |
| `detectors/base.rs`               | Detection        | `Detector` trait definition                     |
| `detectors/custom_rules.rs`       | Detection        | TOML rule-based artifact detection              |
| `detectors/containers.rs`         | Detection        | Docker/compose artifact detection               |
| `detectors/mcp_configs.rs`        | Detection        | MCP server config detection                     |
| `detectors/browser_footprints.rs` | Detection        | Browser extension artifact detection            |
| `rule_engine.rs`                  | Rules            | TOML rule loading and validation                |
| `rules.rs`                        | Rules            | CLI rule management (add/remove/list)           |
| `risk_engine.rs`                  | Analysis         | Signal/type-based risk scoring                  |
| `verifier.rs`                     | Analysis         | Governance verification (pass/conditional/fail) |
| `capabilities.rs`                 | Analysis         | Keyword-to-capability mapping                   |
| `models.rs`                       | Core             | `ArtifactReport`, `ScanReport` types            |
| `scan.rs`                         | Orchestration    | Scan pipeline coordinator                       |
| `contract/`                       | Contract         | Raw artifacts → v2 contract payload             |
| `output.rs`                       | Output           | Local output, JSON files, and submission orchestration |
| `formatters.rs`                   | Output           | ANSI terminal formatters                        |
| `lite_mode.rs`                    | Output           | Result limiting and local scoring               |
| `payload.rs`                      | Output           | Legacy v1 payload builder                       |
| `submit.rs`                       | Side-Effect      | Auth config persistence and HTTP submission     |
| `network.rs`                      | Side-Effect      | Endpoint validation                             |
| `network_evidence.rs`             | Side-Effect      | Host network evidence gathering                 |
| `updater.rs`                      | Side-Effect      | Signed self-update verification and swap        |
| `contract_sync.rs`                | Side-Effect      | Contract schema fetch/cache                     |
| `identity.rs`                     | Side-Effect      | Scanner UUID persistence                        |
