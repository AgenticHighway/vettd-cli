#!/usr/bin/env bash
#
# rename-apply.sh — apply the [AUTO] (mechanical) replacements for the
# proov → vettd-cli rename (issue #119).
#
# Handles ONLY unambiguous, fixed-target substitutions. The binary-vs-product
# distinction (bare `proov`), the crate directory + Cargo metadata renames, the
# clap command name, prose/marketing, the legacy `ah-scan` updater check, the
# `ah-scanner-releases` S3 bucket and the Homebrew formula are intentionally left
# for manual cleanup (verified afterwards by rename-validate.sh).
#
# Idempotent and safe to re-run. Operates on tracked text files only.
#
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

mapfile -t files < <(git ls-files \
    ':!:scripts/rename-validate.sh' \
    ':!:scripts/rename-apply.sh')

changed=0
for f in "${files[@]}"; do
    [ -f "$f" ] || continue
    grep -Iq . "$f" || continue   # skip binary files
    before=$(cat "$f")
    perl -0pi -e '
        s{AgenticHighway/proov}{AgenticHighway/vettd-cli}g;  # repo URL
        s{PROOV_}{VETTD_}g;                                  # env var prefix
        s{AH_SCANNER}{VETTD_SCANNER}g;                       # AH_SCANNER_UUID env var
        s{proov-}{vettd-}g;                                  # artifact / output-file prefixes
        s{ahscan}{vettd}g;                                   # ~/.ahscan, ~/.config/ahscan, .ahscan.toml, ahscan_dir()
        s{Proov}{Vettd}g;                                    # capitalized display name
    ' "$f"
    [ "$(cat "$f")" != "$before" ] && changed=$((changed + 1))
done

echo "[AUTO] replacements applied — $changed of ${#files[@]} tracked files changed."
