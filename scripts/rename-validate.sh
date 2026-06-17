#!/usr/bin/env bash
#
# rename-validate.sh — read-only audit for the proov → vettd-cli rename (issue #119).
#
# Greps the whole repo (including dotfiles, excluding .git/ and this rename-script
# pair) for any remaining legacy names, and labels every finding:
#
#   [AUTO]   — a mechanical, fixed-target substitution handled by rename-apply.sh
#              (repo URLs, PROOV_/AH_SCANNER env vars, proov-* artifact/file
#              prefixes, ~/.ahscan path fragments, the Proov display name).
#   [MANUAL] — needs human judgment: the bare `proov` token (product → vettd-cli
#              vs binary/command → vettd), crate/Cargo renames, the legacy
#              `ah-scan` updater check, the `ah-scanner-releases` S3 bucket, and
#              the Homebrew formula.
#
# A finding is [MANUAL] iff, after conceptually applying the AUTO substitutions,
# a legacy token still remains on the line. Exits non-zero while ANY finding
# remains, so this script doubles as the "zero findings" goal criterion.
#
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Legacy tokens (case-insensitive): product / binary / config / env fragments.
LEGACY='proov|ahscan|ah-scan|ah_scan'

auto=0
manual=0
auto_lines=()
manual_lines=()

while IFS= read -r line; do
    # rg emits "path:lineno:content"; peel the first two colon-delimited fields.
    text=${line#*:}
    text=${text#*:}
    # Remove everything an AUTO rule would handle, then test for a residual token.
    residual=$(printf '%s' "$text" | sed -E '
        s#AgenticHighway/proov##g;
        s#PROOV_##g;
        s#AH_SCANNER##g;
        s#proov-##g;
        s#[Aa]hscan##g;
        s#Proov##g;
    ')
    if printf '%s' "$residual" | grep -iqE "$LEGACY"; then
        manual=$((manual + 1))
        manual_lines+=("$line")
    else
        auto=$((auto + 1))
        auto_lines+=("$line")
    fi
done < <(rg -i -n --no-heading --hidden \
            --glob '!.git/**' \
            --glob '!scripts/rename-validate.sh' \
            --glob '!scripts/rename-apply.sh' \
            -e "$LEGACY" || true)

echo "===================== [AUTO] findings ($auto) ====================="
[ "$auto" -gt 0 ] && printf '%s\n' "${auto_lines[@]}"
echo
echo "==================== [MANUAL] findings ($manual) ===================="
[ "$manual" -gt 0 ] && printf '%s\n' "${manual_lines[@]}"
echo

total=$((auto + manual))
echo "SUMMARY: $total finding(s) — $auto [AUTO], $manual [MANUAL]"
if [ "$total" -gt 0 ]; then
    echo "✗ Rename incomplete: $total legacy reference(s) remain."
    exit 1
fi
echo "✓ No legacy references remain."
