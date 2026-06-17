# Detectors

Detectors are the core of what vettd does — they examine filesystem candidates and produce `ArtifactReport`s describing what they found.

## How detection works

1. **Discovery** (`discovery.rs`) walks tiered scan surfaces or explicit targets and produces a list of `Candidate`s (file paths with origin tags)
2. **Detectors** receive the candidate list and examine each file
3. Each detector returns zero or more `ArtifactReport`s with signals, confidence scores, and metadata
4. Reports are then scored by `risk_engine.rs` and verified by `verifier.rs`
5. Verified reports are formatted for terminal output (`formatters.rs`), serialized to JSON (`contract/`), or submitted to the server (`submit.rs`)

## File primitives

Every file-backed artifact includes **file primitives** — core filesystem metadata gathered once at detection time so downstream consumers (contract builder, formatters, submit) never need to re-read the file:

| Key               | Type   | Description                         |
| ----------------- | ------ | ----------------------------------- |
| `file_size_bytes` | number | File size in bytes                  |
| `last_modified`   | string | ISO-8601 RFC 3339 modification time |
| `content_hash`    | string | SHA-256 hex digest of full file     |
| `paths`           | array  | Absolute file path(s)               |

These are produced by `gather_file_primitives()` in `models.rs` and merged into every artifact's metadata map.

## Built-in detectors

All built-in detectors live in `crates/vettd-cli/src/detectors/` and implement the `Detector` trait:

```rust
pub trait Detector {
    fn name(&self) -> &'static str;
    fn detect(&self, candidates: &[Candidate], deep: bool) -> Vec<ArtifactReport>;
}
```

### Custom Rules (rule engine)

**Detects:** `.cursorrules`, `agents.md`, `AGENTS.md`, `copilot-instructions.md`, `*.prompt.md`, `*.instructions.md`, and any user-installed TOML rules.

Rule-based detection handles all prompt/instruction file types. Each rule defines filename patterns, keyword lists, and confidence thresholds in a declarative TOML file.

**Metadata contract:**

| Key               | Type   | Always present? | Description        |
| ----------------- | ------ | --------------- | ------------------ |
| `file_size_bytes` | number | Yes             | File primitive     |
| `last_modified`   | string | Yes             | File primitive     |
| `content_hash`    | string | Yes             | File primitive     |
| `paths`           | array  | Yes             | Absolute file path |
| `rule_name`       | string | Yes             | Which rule matched |

### MCP Configs (`mcp_configs.rs`)

**Detects:** `mcp.json`, `mcp_config.json`, `claude_desktop_config.json`, `mcp-config.json`, `mcp_settings.json`

Model Context Protocol server configurations. The detector validates JSON structure and extracts server inventory, execution tokens, and credential references.

**Metadata contract:**

| Key                | Type     | Always present?  | Description                            |
| ------------------ | -------- | ---------------- | -------------------------------------- |
| `file_size_bytes`  | number   | Yes              | File primitive                         |
| `last_modified`    | string   | Yes              | File primitive                         |
| `content_hash`     | string   | Yes              | File primitive                         |
| `paths`            | array    | Yes              | Absolute file path                     |
| `server_count`     | number   | Yes              | Number of declared MCP servers         |
| `server_names`     | string[] | If servers exist | Individual server name keys            |
| `execution_tokens` | string[] | If found         | Execution tokens (npx, uvx, python...) |
| `shell_tokens`     | string[] | If found         | Shell access tokens (bash, sh -c...)   |
| `api_endpoints`    | string[] | If found         | URLs extracted from config             |

### Container Configs (`containers.rs`)

**Detects:** `Dockerfile`, `compose.yaml`, `compose.yml`, `docker-compose.yaml`, `docker-compose.yml`

Container-related configuration files that may describe AI execution environments. Dockerfiles are treated as image definitions and compose files are treated as service orchestration. The detector scans file content directly, records proximity to other AI artifacts as a supporting signal, and only upgrades a Docker artifact to `container_config` when the file itself contains AI-related content. Proximity alone remains a weaker `container_candidate` signal.

`agenticApps` promotion is stricter than raw container detection: a Docker artifact only becomes an app contract record when it has direct agentic content or real co-located agent artifacts.

**Metadata contract (Dockerfile):**

| Key                       | Type     | Always present?   | Description                                        |
| ------------------------- | -------- | ----------------- | -------------------------------------------------- |
| `file_size_bytes`         | number   | Yes               | File primitive                                     |
| `last_modified`           | string   | Yes               | File primitive                                     |
| `content_hash`            | string   | Yes               | File primitive                                     |
| `paths`                   | array    | Yes               | Absolute file path                                 |
| `container_kind`          | string   | Yes               | `image_definition` for Dockerfiles                 |
| `direct_ai_evidence`      | boolean  | Yes               | Whether the file itself contains AI tokens         |
| `direct_agentic_evidence` | boolean  | Yes               | Whether the file contains stronger agentic signals |
| `ai_artifact_proximity`   | boolean  | Yes               | Whether nearby AI artifacts were found             |
| `base_image`              | string   | If FROM present   | First FROM image:tag                               |
| `exposed_ports`           | string[] | If EXPOSE present | Port numbers from EXPOSE statements                |

**Metadata contract (compose):**

| Key                       | Type     | Always present? | Description                                        |
| ------------------------- | -------- | --------------- | -------------------------------------------------- |
| `file_size_bytes`         | number   | Yes             | File primitive                                     |
| `last_modified`           | string   | Yes             | File primitive                                     |
| `content_hash`            | string   | Yes             | File primitive                                     |
| `paths`                   | array    | Yes             | Absolute file path                                 |
| `container_kind`          | string   | Yes             | `service_orchestration` for compose-style files    |
| `direct_ai_evidence`      | boolean  | Yes             | Whether the file itself contains AI tokens         |
| `direct_agentic_evidence` | boolean  | Yes             | Whether the file contains stronger agentic signals |
| `ai_artifact_proximity`   | boolean  | Yes             | Whether nearby AI artifacts were found             |
| `services`                | string[] | If found        | Top-level service names                            |

### Source Risks (`source_risks.rs`)

**Detects:** bounded source trees and selected JSON config files in `workdir` and `file` scan modes

This detector emits a single aggregated `source_risk_surface` artifact instead of one artifact per line-level finding. In `workdir` mode it only runs when the candidate set is AI-adjacent. In `file` mode it always analyzes the targeted file if it is a supported source or JSON config.

Current bounded behavior:

- considers at most 512 supported source/JSON candidates per scan
- scans source and JSON content up to 256 KiB per file
- scans JSON config content up to 256 KiB per file
- skips noisy JSON metadata files such as `package.json`, `package-lock.json`, `tsconfig.json`, and `openclaw.plugin.json`
- reuses the shared secret engine plus JSON-specific heuristics for credential-shaped key/value pairs, embedded credential connection strings, metadata/localhost URLs, internal-only URLs, and known collector/C2 destinations
- adds bounded source heuristics for non-literal `import()` / `require()` / process execution, network-context private or internal SSRF targets, sensitive local credential paths, and cognitive identity-file targeting or writes

**Metadata contract:**

| Key                         | Type     | Always present? | Description                                              |
| --------------------------- | -------- | --------------- | -------------------------------------------------------- |
| `paths`                     | array    | Yes             | Aggregated root path for the scanned surface             |
| `matched_families`          | string[] | Yes             | Matched finding families such as `json_secret`           |
| `finding_counts`            | object   | Yes             | Count of findings by family                              |
| `top_risky_files`           | string[] | Yes             | Up to 5 files with the most findings                     |
| `scanned_source_file_count` | number   | Yes             | Number of supported source files included in the surface |
| `scanned_json_file_count`   | number   | Yes             | Number of scannable JSON configs included in the surface |
| `ai_adjacent_context`       | boolean  | Yes             | Whether an AI-adjacent artifact was present in the scan  |
| `bounded_scan_limit`        | number   | Yes             | Maximum supported candidates considered by the detector  |
| `truncated`                 | boolean  | Yes             | Whether candidate selection hit the detector limit       |

### Browser Footprints (`browser_footprints.rs`)

**Detects:** Chrome, Edge, Brave, Arc extension directories

This detector is unique — it **only checks for the presence** of browser profile directories. It never reads extension content or user data. This is a privacy-first design choice.

- Only runs in host/scan/root/filesystem/home scan modes (not in project scans)
- Confidence: fixed 0.6 (presence-only)

**Metadata contract:**

| Key               | Type     | Always present? | Description                        |
| ----------------- | -------- | --------------- | ---------------------------------- |
| `paths`           | array    | Yes             | Extensions directory path          |
| `extension_count` | number   | Yes             | Number of extension subdirectories |
| `extension_ids`   | string[] | Yes             | Extension directory names          |
| `profile_root`    | string   | Yes             | Browser profile root path          |

Note: Browser artifacts do not include file primitives since they represent directory presence, not individual files.

## The Detector trait

The `detect()` method receives:

- `candidates: &[Candidate]` — files to examine
- `deep: bool` — whether to do deeper content analysis (slower but more thorough)

Each candidate has:

- `path` — absolute file path
- `origin` — where it came from (`host`, `home`, `workdir`, `filesystem`, or `root`)

### Content reading limit

Most detectors respect an 8 KB content limit for keyword scanning. The `source_risks` detector is the current exception: it does not do generic keyword scanning, but it will read individual supported source and JSON files up to 256 KiB to run bounded source-analysis, secret, and suspicious-URL heuristics. File primitives (`content_hash`, `file_size_bytes`) are still computed from the complete file.

## Signals

Signals are string tags that describe what a detector found. They follow a naming convention:

| Pattern                      | Example                            | Meaning                              |
| ---------------------------- | ---------------------------------- | ------------------------------------ |
| `filename_match:<name>`      | `filename_match:.cursorrules`      | File matched by name                 |
| `keyword:<word>`             | `keyword:shell`                    | Capability keyword found in content  |
| `dangerous_keyword:<word>`   | `dangerous_keyword:exfiltrate`     | High-risk keyword found              |
| `dangerous_combo:<combo>`    | `dangerous_combo:shell+network+fs` | Multiple risky capabilities together |
| `credential_exposure_signal` | `credential_exposure_signal`       | Possible secret/token detected       |
| `execution_tokens_present`   | `execution_tokens_present`         | Execution binary refs in MCP config  |
| `shell_access_detected`      | `shell_access_detected`            | Shell access in MCP config           |
| `credential_references`      | `credential_references`            | Credential keywords in MCP config    |
| `ai_artifact_proximity`      | `ai_artifact_proximity`            | Container near AI artifact files     |
| `ai_token:<token>`           | `ai_token:langchain`               | AI-relevance token in container file |
| `source:<kind>`              | `source:nonliteral_spawn`          | Aggregated source-analysis heuristic |
| `json_config:<kind>`         | `json_config:c2_url`               | Aggregated JSON config heuristic     |
| `cognitive_tampering:<kind>` | `cognitive_tampering:file_write`   | Source or prompt tampering signal    |

## Adding a new built-in detector

1. Create a new file in `crates/vettd-cli/src/detectors/`, e.g., `my_detector.rs`

2. Implement the `Detector` trait:

    ```rust
    use crate::detectors::base::{Candidate, Detector};
    use crate::models::ArtifactReport;

    pub struct MyDetector;

    impl Detector for MyDetector {
        fn name(&self) -> &'static str {
            "my_detector"
        }

        fn detect(&self, candidates: &[Candidate], deep: bool) -> Vec<ArtifactReport> {
            let mut results = Vec::new();
            for candidate in candidates {
                // Your detection logic here
                // Check candidate.path, read content, pattern match
            }
            results
        }
    }
    ```

3. Register in `detectors/mod.rs`:

    ```rust
    mod my_detector;
    // Add to get_all_detectors() function
    ```

4. Add risk weights in `risk_engine.rs` for your new artifact type

5. Run `cargo test` to verify nothing breaks

## Adding a custom detection rule

If you want detection logic that can be distributed independently (no code changes needed), see [docs/custom-rules.md](custom-rules.md).
