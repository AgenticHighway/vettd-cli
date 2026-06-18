# Deployment & Release Guide

This document covers how to cut a release of vettd, what happens behind the scenes, common problems, and how to fix them.

## Prerequisites

Before releasing, make sure:

- [ ] You have push access to `main` and can create tags
- [ ] You have access to the [GitHub Actions](https://github.com/AgenticHighway/vettd-cli/actions) dashboard to monitor builds
- [ ] All CI checks on `main` are green
- [ ] You've tested the changes locally with `cargo test` and `cargo clippy --all-targets -- -D warnings`
- [ ] GitHub repo variable `VETTD_UPDATE_PUBLIC_KEY_DER_B64` is set to the official KMS public key (base64-encoded DER/SPKI blob)
- [ ] The `SCANNER_RELEASE_ROLE_ARN` OIDC role can call `kms:Sign` on the Vettd release signing key
- [ ] GitHub repo secret `HOMEBREW_TAP_TOKEN` is set if releases should auto-refresh `AgenticHighway/homebrew-tap`

## Release process

### 1. Decide what changed

Check what's been committed since the last release:

```bash
git log --oneline $(git describe --tags --abbrev=0)..HEAD
```

Use [Conventional Commits](https://www.conventionalcommits.org/) to decide the version bump:

| Change type                         | Bump          | Example                |
| ----------------------------------- | ------------- | ---------------------- |
| Breaking changes                    | Major (X.0.0) | API contract schema v3 |
| New features, new detectors         | Minor (0.X.0) | New MCP detector       |
| Bug fixes, dependency updates, docs | Patch (0.0.X) | Fix clippy warning     |

### 2. Bump the version

The version lives in **one place** — the workspace `Cargo.toml`:

```bash
# Edit this line:
# version = "0.6.0"
vim Cargo.toml
```

**Important:** The crate `Cargo.toml` at `crates/vettd-cli/Cargo.toml` uses `version.workspace = true`, so it inherits automatically. Do **not** set the version in the crate's `Cargo.toml`.

### 3. Check if COMPILED_CONTRACT_VERSION needs updating

If the scan output format changed (new fields, removed fields, schema version bump), update the constant in `crates/vettd-cli/src/contract_sync.rs`:

```rust
pub const COMPILED_CONTRACT_VERSION: &str = "2.1.0";
```

This must match the version the server expects. If you're only fixing bugs or updating CI, skip this step.

### 4. Commit the version bump

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: release vX.Y.Z"
```

Include any other files that changed as part of the release (e.g., `contract_sync.rs` if you bumped the contract version).

### 5. Create and push the tag

```bash
git tag vX.Y.Z
git push origin main --tags
```

**The tag name must start with `v`** — the release workflow triggers on `v*` tags only.

### 6. Monitor the release

Go to [GitHub Actions](https://github.com/AgenticHighway/vettd-cli/actions) and watch the Release workflow. It always runs three jobs, plus a fourth when `HOMEBREW_TAP_TOKEN` is configured:

1. **build** — Cross-compiles for 5 targets (runs in parallel):
    - `aarch64-apple-darwin` (macOS ARM64) — GitHub-hosted runner
    - `x86_64-apple-darwin` (macOS x86) — GitHub-hosted runner
    - `aarch64-unknown-linux-gnu` (Linux ARM64) — Blacksmith runner
    - `x86_64-unknown-linux-gnu` (Linux x86) — Blacksmith runner
    - `x86_64-pc-windows-msvc` (Windows x86) — Blacksmith runner

2. **release** — Downloads all 5 artifacts and creates a GitHub Release with auto-generated release notes

3. **upload-s3** — Uploads binaries to `s3://vettd-releases/vX.Y.Z/`, generates SHA-256 checksums, writes `latest.json`, asks AWS KMS to sign it, and uploads both `latest.json` and `latest.signature.json`

4. **update-homebrew-tap** — Computes the macOS/Linux SHA-256 hashes from the release artifacts and pushes the matching `Formula/vettd.rb` update to `AgenticHighway/homebrew-tap`

### 7. Verify the release

After the workflow completes:

```bash
# Check the GitHub Release page exists with all 5 binaries
open https://github.com/AgenticHighway/vettd-cli/releases/tag/vX.Y.Z

# Check the Homebrew tap formula was refreshed
open https://github.com/AgenticHighway/homebrew-tap/blob/main/Formula/vettd.rb

# Check the public hosted manifest and detached signature
curl -s https://vettd.agentichighway.ai/api/scanner/latest | python3 -m json.tool
curl -s https://vettd.agentichighway.ai/api/scanner/latest/signature

# Verify the self-updater works (from an older installed binary)
vettd update --check
```

## What the release workflow does

```
Tag push (v*)
    │
    ▼
┌─────────────────────────────────────────────┐
│ build (5 parallel matrix legs)              │
│                                             │
│  checkout → install rust → cache → build    │
│  → package (.tar.gz / .exe) → upload artifact│
└──────────────────┬──────────────────────────┘
                   │
          ┌────────┴────────┐
          ▼                 ▼
┌─────────────────┐  ┌──────────────────┐
│ release         │  │ upload-s3        │
│                 │  │                  │
│ download all    │  │ download all     │
│ artifacts       │  │ artifacts        │
│ create GitHub   │  │ OIDC → AWS creds │
│ Release         │  │ upload to S3     │
│                 │  │ SHA-256 checksums│
│                 │  │ write latest.json│
│                 │  │ sign manifest    │
│                 │  │ upload signature │
└─────────────────┘  └──────────────────┘
```

**Security notes:**

- All Actions are pinned to full commit SHAs (not mutable tags)
- AWS credentials use OIDC federation — no long-lived keys in secrets
- Official release builds embed the public update verification key at compile time
- `latest.json` is signed by AWS KMS and verified by official Vettd binaries before hashes are trusted
- SHA-256 checksums remain embedded in `latest.json` for per-artifact integrity verification after the manifest signature passes

## Pre-release checklist

```
□ cargo fmt --check                          passes
□ cargo clippy --all-targets -- -D warnings  passes
□ cargo test                                 337+ tests pass
□ cargo deny check                           no advisory/license issues
□ cargo audit                                no known vulnerabilities
□ Version bumped in workspace Cargo.toml
□ COMPILED_CONTRACT_VERSION correct (if schema changed)
□ VETTD_UPDATE_PUBLIC_KEY_DER_B64 repo variable configured
□ Release OIDC role allowed to call kms:Sign
□ All changes committed, working tree clean
□ CI green on main
```

## Common problems and resolutions

### Build fails on one platform

**Symptom:** 4 of 5 matrix legs succeed, one fails.

**Common causes:**

- **Linux ARM64 cross-compile fails:** The `gcc-aarch64-linux-gnu` package may have changed. Check the "Install cross-compilation tools" step logs.
- **Windows build fails:** Windows runners can have transient issues. Re-run the failed job from the Actions UI.
- **macOS build fails:** Often a Rust toolchain installation issue with GitHub-hosted runners. Re-run.

**Resolution:** Use the "Re-run failed jobs" button on the Actions page. If the failure is deterministic, investigate the build logs.

### macOS cross-compilation fails: "can't find crate for core"

**Symptom:** The `x86_64-apple-darwin` build fails with:

```
error[E0463]: can't find crate for `core`
  = note: the `x86_64-apple-darwin` target may not be installed
```

**Cause:** `macos-latest` runners are ARM64 (Apple Silicon). Building for `x86_64-apple-darwin` is a cross-compilation that requires the target to be explicitly installed. If `rust-toolchain.toml` exists, it overrides `dtolnay/rust-toolchain` action inputs — the action's `targets:` parameter is silently ignored.

**Resolution:** The release workflow includes an explicit `rustup target add ${{ matrix.target }}` step to ensure cross-compilation targets are always installed regardless of `rust-toolchain.toml`. If this step is missing or removed, add it back after the "Install Rust toolchain" step.

_This was the root cause of the v0.6.1 initial release failure (2026-03-31)._

### CI fails: "cargo-fmt is not installed"

**Symptom:** The formatting check fails with:

```
error: 'cargo-fmt' is not installed for the toolchain '1.85.1-x86_64-unknown-linux-gnu'
```

**Cause:** Same `rust-toolchain.toml` override issue — the `components: clippy, rustfmt` input on `dtolnay/rust-toolchain` is ignored. Components must be listed in `rust-toolchain.toml` directly.

**Resolution:** Ensure `rust-toolchain.toml` includes:

```toml
components = ["clippy", "rustfmt"]
```

The CI workflow also has a belt-and-suspenders `rustup component add clippy rustfmt` step.

### CI fails: cargo-deny or cargo-audit installation fails

**Symptom:** The supply chain audit job fails during binary installation with tar errors.

**Common causes:**

- **cargo-deny:** The download URL must include the version in the filename (e.g., `cargo-deny-0.19.0-x86_64-...`). A URL without the version returns an HTML page, not a binary.
- **cargo-audit:** The `rustsec/rustsec` repo is a monorepo — `/releases/latest` returns whichever crate released most recently (often `platforms`, not `cargo-audit`). The CI must search for the latest `cargo-audit/*` tag specifically.

**Resolution:** The CI workflow constructs download URLs dynamically by fetching the correct release tag first. If these scripts break, check:

1. Has the release asset naming convention changed?
2. Has the GitHub API response format changed?
3. Run the URL construction commands locally to debug.

### Tag was pushed but workflow didn't trigger

**Symptom:** You pushed a tag but no workflow run appears.

**Causes:**

- The tag name doesn't match `v*` (e.g., you used `0.6.1` instead of `v0.6.1`)
- The tag was created on a branch other than what's expected

**Resolution:**

```bash
# Delete the bad tag
git tag -d bad-tag
git push origin :refs/tags/bad-tag

# Create correctly
git tag vX.Y.Z
git push origin --tags
```

### S3 upload fails

**Symptom:** GitHub Release was created but `latest.json` wasn't updated.

**Causes:**

- OIDC token exchange failed (transient GitHub/AWS issue)
- The `SCANNER_RELEASE_ROLE_ARN` secret is misconfigured
- The release role is missing `kms:Sign` permission or the signing key alias is wrong

**Resolution:**

1. Check the "Configure AWS credentials (OIDC)" step logs for the error
2. Check the signing step logs for `aws kms sign` errors or missing signing config
3. Re-run the `upload-s3` job from the Actions UI
4. If OIDC is persistently failing, verify the IAM role trust policy allows the repo's OIDC subject

### SHA-256 checksums are empty in latest.json

**Symptom:** `latest.json` has empty string values for `sha256` fields.

**Cause:** The `checksums` step output variable names didn't match. This happens if artifact filenames change.

**Resolution:** Check that artifact names in the matrix match what the `Write latest.json` step expects. The variable names are derived by replacing non-alphanumeric characters with underscores.

### Users report `vettd update` doesn't find the new version

**Symptom:** The release is on GitHub but `vettd update --check` says "already up to date."

**Causes:**

- `latest.json` on S3 wasn't updated (S3 upload job failed or was skipped)
- Client-side 24-hour check cache hasn't expired

**Resolution:**

```bash
# Verify latest.json
curl -s https://vettd.agentichighway.ai/api/scanner/latest

# If it shows the old version, re-run the upload-s3 job

# To bypass the client cache:
rm -f ~/.vettd/update_check_cache
vettd update --check
```

### Need to re-release the same version

**Symptom:** A release was published but had a bug. You need to re-do it.

**Resolution:**

```bash
# Delete the tag locally and remotely
git tag -d vX.Y.Z
git push origin :refs/tags/vX.Y.Z

# Delete the GitHub Release from the web UI (Releases → edit → delete)

# Fix the issue, commit
git add . && git commit -m "fix: <description>"

# Re-tag and push
git tag vX.Y.Z
git push origin main --tags
```

### Cargo.lock is out of sync after version bump

**Symptom:** `cargo build` modifies `Cargo.lock` after you changed the version in `Cargo.toml`.

**Resolution:** Always run `cargo check` or `cargo build` after bumping the version, then commit both files:

```bash
# After editing Cargo.toml
cargo check
git add Cargo.toml Cargo.lock
git commit -m "chore: release vX.Y.Z"
```

## Rollback

If a release is broken and users are affected:

1. **Immediate:** Re-upload the previous version's manifest and detached signature envelope so `vettd update` trusts the previous release again:

    ```bash
    aws s3 cp s3://vettd-releases/manifests/vPREVIOUS/latest.json s3://vettd-releases/latest.json
    aws s3 cp s3://vettd-releases/manifests/vPREVIOUS/latest.signature.json s3://vettd-releases/latest.signature.json
    ```

2. **Thorough:** Follow the "re-release" steps above to publish a fixed version.

Users who haven't run `vettd update` yet won't be affected — the binary is self-contained.
