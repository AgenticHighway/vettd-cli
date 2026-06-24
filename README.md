# vettd

[![Version](https://img.shields.io/github/v/release/AgenticHighway/vettd-cli?label=version)](https://github.com/AgenticHighway/vettd-cli/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/AgenticHighway/vettd-cli/ci.yml?branch=main&label=ci)](https://github.com/AgenticHighway/vettd-cli/actions/workflows/ci.yml)
[![Tests](https://img.shields.io/github/actions/workflow/status/AgenticHighway/vettd-cli/ci.yml?branch=main&label=tests)](https://github.com/AgenticHighway/vettd-cli/actions/workflows/ci.yml)
[![Security](https://img.shields.io/github/actions/workflow/status/AgenticHighway/vettd-cli/ci.yml?branch=main&label=security)](https://github.com/AgenticHighway/vettd-cli/actions/workflows/ci.yml)
[![Fmt + Lint](https://img.shields.io/github/actions/workflow/status/AgenticHighway/vettd-cli/ci.yml?branch=main&label=fmt%20%2B%20lint)](https://github.com/AgenticHighway/vettd-cli/actions/workflows/ci.yml)

**Detect, analyze, and report AI execution artifacts on a host machine.**

vettd is a Rust CLI tool that scans your system for AI-related configuration files — things like `.cursorrules`, MCP server configs, prompt files, and container definitions — analyzes them for risk, and produces structured reports.

## Support Vettd

If Vettd helps your team, you can support ongoing open source development with a donation via Stripe:

[Donate to Vettd](https://donate.stripe.com/fZu00cdxZcxAbHH2Ya2ZO00)

## How it works

vettd is local-first. It walks your filesystem, identifies AI execution artifacts, scores them for risk, and writes results locally. Network activity only happens when you explicitly opt into submission-related flows or `vettd update`. `vettd auth` and `vettd setup` only save local configuration.

If you want a hosted review, policy, and governance surface, vettd can
submit to compatible ingest APIs. You can configure an endpoint during
setup or pass one directly at submission time.

## System requirements

| Requirement | Detail                                                                                  |
| ----------- | --------------------------------------------------------------------------------------- |
| **OS**      | macOS (ARM64, x86_64), Linux (ARM64, x86_64), Windows (x86_64)                          |
| **Runtime** | None — official releases are single binaries with no sidecar service                    |
| **Build**   | Rust 1.85.1+ (pinned via `rust-toolchain.toml`)                                         |
| **Network** | Optional — only needed for submission-related flows and self-update                     |
| **Disk**    | ~15-25 MB for the binary depending on platform; reports are only written when requested |

## Install

### Homebrew (recommended on macOS)

```bash
brew tap AgenticHighway/tap
brew install vettd
vettd quick
```

Homebrew is the smoothest install path on macOS while direct-download
artifacts are not yet signed and notarized.

### From a release binary

Download the latest binary for your platform from [GitHub Releases](https://github.com/AgenticHighway/vettd-cli/releases).

```bash
# macOS (Apple Silicon)
tar xzf vettd-darwin-arm64.tar.gz
./vettd quick

# Linux (x86_64)
tar xzf vettd-linux-amd64.tar.gz
./vettd quick
```

#### Verifying downloads

Each GitHub Release includes `checksums.txt` (SHA-256 hashes for all
release assets) and `checksums.txt.sig` (a KMS ECDSA signature over
`checksums.txt`, using the same key that `vettd update` trusts).

```bash
# Download the binary and checksums for your platform, then:

# Linux — verify SHA-256
sha256sum --check --ignore-missing checksums.txt

# macOS — verify SHA-256
shasum -a 256 --check --ignore-missing checksums.txt
```

For the full signature-verification procedure (including how to verify
`checksums.txt.sig` with the embedded public key), see
[docs/release-verification.md](docs/release-verification.md).

### From source

```bash
git clone https://github.com/AgenticHighway/vettd-cli.git
cd vettd
cargo build --release
./target/release/vettd quick
```

## Quick start

```bash
vettd                      # Interactive wizard — walks you through options
vettd quick                # Tier 1 scan of critical AI config areas (~/.cursor, VS Code, Claude, etc.)
vettd scan                 # Default tiered scan of critical roots + bounded user-space/project roots
vettd full                 # Deep system-wide scan (slow, thorough)
vettd file <path>          # Scan a single file
vettd folder <path>        # Scan a directory
vettd repo <path>          # Deep-scan a git repository
vettd setup                # Interactive connected-mode setup (API key + endpoint)
vettd auth                 # Prompt securely for an API key and save it
vettd auth --key <key>     # Save API credentials directly (useful for automation)
vettd auth status          # Show auth/identity status (coming soon)
vettd contract status      # Show local vs. server contract status (coming soon)
vettd directory <cmd>      # Browse the public directory: search|list|trending|random|view|findings|compare (coming soon)
vettd update               # Check for and install updates
vettd rules list           # List installed custom detection rules
vettd rules add <file>     # Install a TOML rule file
vettd rules remove <name>  # Remove an installed rule by name
vettd rules validate <f>   # Validate a rule file without installing
```

## Scan surfaces

`vettd` now treats scan modes as scan-surface tiers instead of simple breadth presets:

- `quick`: Tier 1 only — critical OS-aware agent surfaces such as VS Code, Cursor, Claude, Continue, Aider, and similar local tool config roots
- `scan`: Tier 1 plus bounded user-space/project roots such as `Desktop`, `Documents`, `Downloads`, and common repo folders like `Code`, `Projects`, `Workspace`, `src`, and `GitHub`
- `folder`: explicit target directory plus bounded local adjacency
- `repo`: explicit target repository plus deeper local adjacency
- `full`: explicit forensic sweep from filesystem root

Current critical roots are OS-aware:

- macOS: `~/Library/Application Support/Code/User`, `~/Library/Application Support/Code - Insiders/User`, `~/Library/Application Support/Cursor/User`, plus `~/.claude`, `~/.cursor`, `~/.aider`, `~/.continue`, `~/.ollama`, and editor config roots when present
- Linux: `~/.config/Code/User`, `~/.config/Code - Insiders/User`, `~/.config/Cursor/User`, plus `~/.claude`, `~/.cursor`, `~/.aider`, `~/.continue`, and `~/.ollama`
- Windows: `%APPDATA%\\Code\\User`, `%APPDATA%\\Code - Insiders\\User`, `%APPDATA%\\Cursor\\User`, plus `%USERPROFILE%\\.claude`, `%USERPROFILE%\\.cursor`, `%USERPROFILE%\\.aider`, `%USERPROFILE%\\.continue`, and `%USERPROFILE%\\.ollama`

## Output formats

```bash
vettd quick              # Overview with risk bars (default)
vettd quick --full       # Detailed per-artifact breakdown
vettd quick --summary    # Compact statistics only
vettd quick --json       # JSON report to stdout
vettd quick --out        # JSON report to ./vettd-report.json
vettd quick --out r.json # JSON report to a custom path
vettd quick --contract   # Scanner data contract JSON to stdout
vettd quick --contract --out r.json  # Contract JSON to file
vettd quick --contract --submit --api-key <key>  # Contract to file + submit
```

`--json`, `--out`, and `--contract` all emit the scanner data contract
shape used for compatible ingest APIs and local automation.

## What it detects

The table below is representative, not exhaustive. vettd combines built-in
detectors, packaged TOML rules, and optional custom rules, so additional file
names and patterns may be detected beyond these common examples. For deeper
detector details, see [docs/detectors.md](docs/detectors.md).

| Detector              | Files                                                         | What it looks for                                                                                    |
| --------------------- | ------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Cursor / editor rules | `.cursorrules`, `agents.md`, `AGENTS.md`                      | AI instruction files with capability keywords (TOML rule)                                            |
| Prompt configs        | `*.prompt.md`, `*.instructions.md`, `copilot-instructions.md` | Prompt configuration for GitHub Copilot and similar (TOML rule)                                      |
| MCP configs           | `mcp.json`, `claude_desktop_config.json`                      | Model Context Protocol server declarations                                                           |
| Container configs     | `Dockerfile`, `compose.yaml`, `docker-compose.yml`            | Docker image definitions and service orchestration with direct AI evidence or nearby agent artifacts |
| Browser footprints    | Chrome, Edge, Brave, Arc profiles                             | Extension directory presence only (no content reads)                                                 |
| Custom rules          | Any `.toml` in `~/.vettd/rules/`                             | Declarative rules you define                                                                         |

## Risk scoring

Every artifact gets a risk score from 0–100:

| Score | Severity | Color   | Meaning                              |
| ----- | -------- | ------- | ------------------------------------ |
| 90+   | CRITICAL | Magenta | Credential exposure or extreme risk  |
| 70-89 | HIGH     | Red     | Dangerous capability combinations    |
| 40-69 | MEDIUM   | Yellow  | Notable capabilities worth reviewing |
| 10-39 | LOW      | Cyan    | Minor signals, likely benign         |
| 0-9   | INFO     | Dim     | Informational only                   |

Scores are based on: artifact type, detected capability keywords (shell, network, filesystem, etc.), dangerous keywords (exfiltrate, steal, bypass, etc.), and whether capabilities are explicitly declared.

## Optional local access mode

By default, vettd shows the full local report.

If you want to locally limit emitted findings to the top three visible
artifacts, add an `.vettd.toml` file in your project root:

```toml
[access]
mode = "lite"
```

When `.vettd.toml` is absent, vettd keeps full output enabled.

## Submitting to a compatible endpoint

vettd supports hosted submission flows through compatible ingest APIs.
You can configure an endpoint during setup or pass one directly for
self-hosting, testing, or interoperability.

With an API key configured:

```bash
# First-time setup (saves credentials and endpoint)
vettd setup

# Or prompt securely for credentials
vettd auth

# Or set credentials directly for automation
vettd auth --key your-api-key

# Submit scan results (uses saved endpoint)
vettd repo . --submit --api-key your-key

# Submit to a custom public endpoint — requires --allow-public-endpoint
vettd repo . --submit https://example.com/api/scans/ingest \
  --allow-public-endpoint --api-key your-key

# Save a custom public endpoint for future use — also requires the flag
vettd auth --key your-api-key \
  --endpoint https://example.com/api/scans/ingest \
  --allow-public-endpoint
```

### What is included in a submission

Each submission payload contains:

- **Scan root paths** — the filesystem paths where the scan ran (e.g. `/Users/you`)
- **Machine hostname** — the host's reported hostname
- **AI artifact metadata** — file paths, content hashes, capability signals, and risk scores for detected artifacts
- **MCP server config metadata** — server commands, tool names, and env-var names (values are never transmitted)
- **Host security context** — macOS firewall state on macOS; empty on other platforms
- **Scanner metadata** — version, OS, and architecture

No file contents, secret values, or credential material are transmitted.

In **interactive flows** (`vettd scan` / `vettd quick` / etc. from a terminal), vettd displays this summary and asks for confirmation before sending.

In **automation flows** (`--submit --api-key …`), the submission proceeds without a prompt. Review the data categories above before embedding `vettd` in CI/CD pipelines.

### `--allow-public-endpoint`

By default, vettd blocks submission to public (non-localhost, non-private-network) endpoints at the flag level. This prevents accidental data exfiltration in scripts that omit an explicit endpoint. Pass `--allow-public-endpoint` when you intentionally want to submit to or save a public endpoint.

Local endpoints (`localhost`, `127.0.0.1`, `::1`, RFC-1918 addresses) are always allowed without the flag.

### Safety defaults

- Scans stay local unless you explicitly opt into connected commands
- Compatible submission endpoints are supported, whether configured ahead of time or passed on the command line
- Contract sync only runs during explicit submission flows, when the target endpoint exposes a compatible contract API
- Retry logic handles transient failures (429, 502, 503, 504)
- On Unix-like systems, saved API keys are written to `~/.config/vettd/config.json` and vettd explicitly tightens directory/file permissions to `0700` / `0600`
- On Windows, saved API keys are written under the current user's config directory, typically `%APPDATA%\\vettd\\config.json`, and currently rely on the default per-user profile ACLs rather than explicit ACL hardening by vettd

## Self-update

```bash
vettd update           # Check for and install updates
vettd update --check   # Check only, don't install
vettd update --force   # Force update even if current
```

`vettd update` explicitly checks for the latest release, verifies the signed
manifest, checks the matching artifact SHA-256, and replaces the local binary.

Official release binaries verify a detached AWS KMS-backed ECDSA signature on
the update manifest before trusting artifact URLs or hashes.

Source builds remain fully functional, but `vettd update` will fail explicitly
unless the binary was built with `PROOV_UPDATE_PUBLIC_KEY_DER_B64` set at
compile time.

## Privacy

- **Path-first scanning** — content is only read from specific allowlisted file types
- **Bounded walking** — max depth of 5 for shallow scans; full scan enumerates the entire filesystem with no caps
- **Scoped exclusions** — `.git/`, `node_modules/`, `.venv/`, `target/` and similar are excluded from default, workdir, and filesystem scans (full scan has no exclusions)
- **Secret detection without storage** — token patterns trigger a signal tag, but values are never stored or transmitted
- **Browser presence only** — extension directories are noted, but no extension content or preferences are read
- **Declarative rules** — custom rules are TOML config files; they use the same content-read allowlist as built-in detectors

## Project structure

```
vettd/
├── crates/
│   └── vettd/                    # CLI binary (scanning, detection, submission)
│       └── src/
│           ├── detectors/         # Built-in artifact detectors
│           └── contract/          # Scanner data contract builders
├── rules/                        # Built-in TOML detection rules (compiled into binary)
├── examples/
│   └── rules/                    # Example custom detection rules (.toml)
├── scripts/
│   ├── test-scanner.sh           # Automated test suite
│   └── test-submit.sh            # Manual submission test
├── docs/
│   ├── architecture.md           # System design and data flow
│   ├── user-flows.md             # Public CLI journeys and UX paths
│   ├── detectors.md              # How detection works
│   ├── output-spec.md            # Plain-English spec for contract outputs
│   └── custom-rules.md           # Writing custom detection rules
├── .github/
│   ├── workflows/
│   │   ├── ci.yml                # PR checks: fmt, clippy, test, audit
│   │   └── release.yml           # Build + GitHub Release + artifact publishing
│   ├── dependabot.yml            # Automated dependency updates
│   └── CODEOWNERS                # Required reviewers for security-sensitive files
├── CODE_OF_CONDUCT.md            # Community expectations and reporting path
├── LICENSE                       # MIT license text
├── agents.md                     # Project guidelines for AI coding agents
├── deny.toml                     # Supply chain policy (licenses, advisories)
├── rust-toolchain.toml           # Pinned Rust compiler version
├── SECURITY.md                   # Vulnerability disclosure policy
└── scanner-data-contract.json    # JSON Schema for scan output
```

## Developing

```bash
cargo build                         # Debug build
cargo fmt --check                   # Formatting check
cargo clippy --all-targets -- -D warnings  # Lint (must be 0 warnings)
cargo test                          # Run the full test suite
cargo deny check                    # License + advisory audit
cargo audit                         # RustSec vulnerability scan
./scripts/test-scanner.sh           # Exercise all CLI subcommands
./scripts/benchmark-scanner.sh      # Record repeatable local scan timings
```

> **CI runs all of these automatically on every PR.** See [.github/workflows/ci.yml](.github/workflows/ci.yml) for the full pipeline.

Set `VETTD_TIMINGS=1` when you want per-stage scan timings on stderr during a
local run or benchmark session.

For detailed development instructions: [CONTRIBUTING.md](CONTRIBUTING.md)
For architecture and code walkthrough: [docs/architecture.md](docs/architecture.md)
For public CLI journeys: [docs/user-flows.md](docs/user-flows.md)
For the plain-English output spec: [docs/output-spec.md](docs/output-spec.md)
For community expectations: [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
For security vulnerability reports: [SECURITY.md](SECURITY.md)

## Configuration reference

| File                           | Purpose                                       |
| ------------------------------ | --------------------------------------------- |
| `~/.config/vettd/config.json` | API key + endpoint (created by `vettd setup`) |
| `.vettd.toml`                 | Optional local access-mode settings           |
| `~/.vettd/scanner_uuid`       | Persistent scanner identity (auto-generated)  |
| `~/.vettd/rules/*.toml`       | Custom detection rules                        |

Optional `.vettd.toml`:

```toml
[access]
mode = "lite"                   # limit visible findings to the top three artifacts
```

## License

Copyright (c) 2026 Agentic Highway.

`vettd` is licensed under the **MIT License**. See [LICENSE](LICENSE) and
[COPYRIGHT](COPYRIGHT).
