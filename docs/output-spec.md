# Output Spec

This document is the plain-English reference for what vettd is trying to produce.

It answers two related questions:

1. What does the scanner detect directly from files on disk?
2. What do the contract outputs mean once those raw findings are transformed into `prompts`, `skills`, `mcpServers`, `agents`, and `agenticApps`?

Use this file as the readable spec for product and review discussions. Use `docs/detectors.md` for low-level detector behavior and `crates/vettd-cli/src/contract/` for the exact implementation.

## Two layers

vettd has two distinct layers.

### Layer 1: Raw artifact detection

The scanner walks files and emits `ArtifactReport` values such as:

- `cursor_rules`
- `prompt_config`
- `agents_md`
- `mcp_config`
- `container_config`
- `container_candidate`
- `browser_footprint`

These are the scanner's direct findings.

### Layer 2: Contract output generation

The contract builder takes those raw artifacts and produces higher-level sections:

- `scanMeta`
- `prompts`
- `skills`
- `mcpServers`
- `agents`
- `agenticApps`

These are not all one-to-one detections. Some are derived views assembled from multiple artifact types.

## Artifact-to-output mapping

This is the current intended mapping.

| Raw artifact type     | Source files                                                            | Contract outputs it can affect                                                   | What it is for                                                                        |
| --------------------- | ----------------------------------------------------------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `cursor_rules`        | `.cursorrules`                                                          | `prompts`, sometimes `skills`                                                    | Editor or tool instruction files that shape AI behavior                               |
| `prompt_config`       | `*.prompt.md`, `*.instructions.md`, `copilot-instructions.md`           | `prompts`, sometimes `skills`                                                    | Prompt or instruction documents used by AI tooling                                    |
| `skill`               | `SKILL.md`, `skill.md`                                                  | `skills`                                                                         | Reusable agent skill instruction files; distinct from prompts though they may contain prompt text |
| `agents_md`           | `agents.md`, `AGENTS.md`                                                | `prompts`, `agents`, sometimes `skills`, sometimes `agenticApps` via co-location | First-class agent definition files                                                    |
| `mcp_config`          | `mcp.json`, `mcp_config.json`, `claude_desktop_config.json`, similar    | `mcpServers`, sometimes `skills`, linked into `agents`                           | MCP server declarations and connection metadata                                       |
| `container_config`    | Dockerfiles or compose files with direct AI content                     | sometimes `agenticApps`                                                          | Container-related files with direct AI evidence                                       |
| `container_candidate` | Dockerfiles or compose files with weaker evidence, often proximity only | sometimes `agenticApps` only if real local agents are present                    | Container-related files that may matter but are not strong enough on their own        |
| `browser_footprint`   | Browser extension directories                                           | no current contract section                                                      | Host-level presence signals used for scan/risk output, not contract entity generation |

## What each output section is for

### `scanMeta`

`scanMeta` is the run-level envelope.

It is set up to answer:

- What machine produced this scan?
- When was it produced?
- What path or paths were scanned?
- What scanner version produced it?
- What host network/firewall context was present?

It is not tied to one artifact. It describes the scan run as a whole.

### `prompts`

`prompts` are set up to represent instruction-bearing files that can steer model behavior.

Current sources:

- `cursor_rules`
- `prompt_config`
- `agents_md`

What a prompt entry is meant to capture:

- the source file
- whether the file behaves more like a system prompt or user prompt
- rough token count
- content hash and modified date
- high-level capabilities inferred from the file
- signs of secret references
- obvious prompt injection or dangerous instruction surfaces

Current classification rules:

- `cursor_rules` and `agents_md` become `System Prompt`
- everything else becomes `User Prompt`

Plain-English intent:

`prompts` answer "what instructions are shaping AI behavior here?"

They are not meant to prove runtime execution. They describe instruction surfaces.

### `skills`

`skills` are set up to represent tools or execution primitives that an agent can call.

Current sources:

- `skill` artifacts detected from `SKILL.md`
- `declared_tools` metadata on any artifact
- MCP server command names extracted from MCP config files

What a skill entry is meant to capture:

- the tool name
- whether it is a CLI tool, local function, or HTTP integration
- the rough execution environment
- a readable description of what the tool does
- required permissions implied by the tool
- binary or API dependencies
- which agents appear to consume it

Important heuristics:

- skills are deduplicated by name
- trust level comes from the risk score of the artifact that introduced the skill
- MCP command-derived skills are always added conservatively as shell-invoked local tools

Plain-English intent:

`skills` answer "what can the detected agents or configs actually use to do work?"

They are not a full SBOM and not a complete OS capability inventory. They are the scanner's best normalized view of actionable tools.

### `mcpServers`

`mcpServers` are set up to represent Model Context Protocol server declarations.

Current source:

- `mcp_config`

What an MCP server entry is meant to capture:

- the logical server name
- the transport type
- the network exposure classification
- how authentication appears to work
- whether the source artifact passed verification
- the effective command used to launch it
- the tools it exposes, either explicit or inferred
- environment variable references
- network evidence
- which agents depend on it

Important heuristics:

- one contract entry is produced per unique MCP server name
- `dependentAgents` is linked after agents are built
- auth is heuristic, not credential validation
- tools are explicit if listed, otherwise inferred from command shape and naming

Plain-English intent:

`mcpServers` answer "what MCP infrastructure is declared here, how risky does it look, and who appears to use it?"

They are not a live connectivity check. They describe declared configuration.

### `agents`

`agents` are set up to represent first-class agent definitions.

Current source:

- `agents_md`

What an agent entry is meant to capture:

- the agent's file and stable ID
- its coarse classification
- whether it looks autonomous or user-in-the-loop
- a trust score derived from risk
- a capability flag summary
- declared tools
- linked MCP tools from nearby MCP configs
- a trust breakdown explaining the biggest positive and negative factors

Current classification rules:

- shell or code execution capability yields `Code`
- container runtime or dependency execution yields `Automation`
- browser or external API capability yields `Research`
- otherwise the default is `System`

Current execution model rules:

- dangerous keywords or dangerous shell+network+filesystem combos yield `Autonomous`
- otherwise the default is `User-in-the-loop`

Plain-English intent:

`agents` answer "what named agent definitions exist, what kind of agents do they appear to be, and what tools do they seem able to use?"

They are not process inspection and not runtime telemetry.

### `agenticApps`

`agenticApps` are set up to represent containerized or orchestrated application groupings that are agentic enough to review as an application, not just as an isolated file.

Current sources:

- `container_config`
- `container_candidate`

But a container artifact is only promoted into `agenticApps` when one of these is true:

- the container file has direct agentic evidence
- real local agent artifacts are co-located with it

What an agentic app entry is meant to capture:

- the source container file and stable ID
- the inferred framework such as LangGraph, CrewAI, AutoGen, or `Custom`
- how many local agents appear attached
- a readable description of what kind of app this looks like
- per-agent tool lists and a coarse workflow summary
- integrations inferred from API endpoints
- verification checks and risk tags
- a human-readable risk summary

Important Docker semantics:

- Dockerfiles are treated as `image_definition`
- compose files are treated as `service_orchestration`
- proximity to AI files alone is not enough to create an app

Plain-English intent:

`agenticApps` answer "what project-level AI application groupings should a reviewer think about as one app?"

They are intentionally stricter than raw container detection to avoid false positives.

## What is direct detection versus derived inference?

This is the most important distinction in the whole system.

### Directly detected

These are discovered straight from files or directories:

- prompt-like files
- MCP config files
- Docker and compose files
- browser footprint directories

### Derived later

These are built by interpretation rules after detection:

- `prompts`
- `skills`
- `mcpServers`
- `agents`
- `agenticApps`
- classifications like `System Prompt`, `Research`, `Autonomous`, `Trusted`, `Custom`, and so on

If something looks surprising in the contract output, the right debugging question is usually:

1. Was the raw artifact detected correctly?
2. Did the contract builder interpret it the way we intended?

## Known non-goals

This spec describes what the system is currently set up for. It is not claiming more than that.

Current non-goals include:

- live process inspection
- validating that a Docker image exists in a registry
- proving that an MCP server is actually reachable at runtime
- reconstructing exact agent execution history
- building a complete inventory of every tool on the host

## Source of truth in code

If this document and the implementation ever diverge, check these files:

- `crates/vettd-cli/src/contract/mod.rs`
- `crates/vettd-cli/src/contract/prompts.rs`
- `crates/vettd-cli/src/contract/skills.rs`
- `crates/vettd-cli/src/contract/mcp.rs`
- `crates/vettd-cli/src/contract/agents.rs`
- `crates/vettd-cli/src/contract/apps.rs`
- `docs/detectors.md`

This file should be updated whenever the inclusion rules or the meaning of a contract section changes.
