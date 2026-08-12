//! Public freshness DTO and display helpers for the Vettd directory API.
//!
//! Wire-tied to slice 1 (`packages/api/src/freshness/serialize.ts`). The DTO
//! shape must stay in sync with `PublicFreshness` on the server side:
//!
//! ```ts
//! interface PublicFreshness {
//!   status: "verified_unchanged" | "changed" | "unreachable" | "check_failed";
//!   reason: string | null;
//!   retryable: boolean;
//!   renamedTo: string | null;
//!   lastCheckedAt: string | null;
//!   lastVerifiedAt: string | null;
//!   lastChangeDetectedAt: string | null;
//!   scannedHash: string | null;
//!   latestUpstreamHash: string | null;
//! }
//! ```
//!
//! `null` freshness means no row exists on the server (asset has never been
//! verified against upstream).

use serde::{Deserialize, Serialize};

// ── DTO ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(non_snake_case)]
pub struct PublicFreshness {
    pub status: String,
    pub reason: Option<String>,
    pub retryable: bool,
    pub renamed_to: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_verified_at: Option<String>,
    pub last_change_detected_at: Option<String>,
    pub scanned_hash: Option<String>,
    pub latest_upstream_hash: Option<String>,
}

// ── Classification ─────────────────────────────────────────────────────────

/// Map a DTO status to a single-word display label used in compact CLI rows.
///
/// `unknown` is used for `null` (no freshness row) so it never merges with
/// `verified_unchanged` — unknown/missing data must never look "current".
pub fn classify_freshness(dto: &Option<&PublicFreshness>) -> &'static str {
    match dto.as_ref() {
        Some(d) => match d.status.as_str() {
            "verified_unchanged" => "current",
            "changed" => "changed",
            "unreachable" => "unreachable",
            _ => "unknown", // check_failed and anything unrecognized
        },
        None => "unknown",
    }
}

/// One-line compact label rendered in list/search/trending/random rows.
///
/// The label is plain text (no ANSI) so callers can wrap it in their own
/// colors. Always ≤ 14 chars — fits in a narrow column.
pub fn fmt_freshness_compact(dto: &Option<&PublicFreshness>) -> String {
    match classify_freshness(dto) {
        "current" => "[ok]".to_string(),
        "changed" => "[chgd]".to_string(),
        "unreachable" => "[offline]".to_string(),
        _ => "[? ]".to_string(),
    }
}

/// ANSI-colored compact label for terminal output.
pub fn fmt_freshness_colored(dto: &Option<&PublicFreshness>) -> String {
    let label = fmt_freshness_compact(dto);
    let color = match classify_freshness(dto) {
        "current" => "\x1b[32m",     // green
        "changed" => "\x1b[33m",     // yellow
        "unreachable" => "\x1b[31m", // red
        _ => "\x1b[2m",              // dim (unknown)
    };
    format!("{color}{label}\x1b[0m")
}

// ── Detail (rich view / compare) ───────────────────────────────────────────

/// Human-readable summary of a freshness DTO for the rich `view` command.
///
/// Lines:
///  1. Freshness status (colored)
///  2. Reason (if any)
///  3. Retryable (if set — lets the user know a failed check can be retried)
///  4. Renamed → X (if any)
///  5. Timestamps (last checked / last verified / last change)
///  6. Hashes (scanned / upstream) — never abbreviated here (lossless)
///
/// Empty sections are omitted so unchanged assets don't show a blank block.
/// When no freshness row exists at all (`None`), an explicit "unknown" line is
/// still rendered — missing data must never be silently mistaken for current.
pub fn fmt_freshness_detail(dto: &Option<&PublicFreshness>) -> Vec<String> {
    let reset = "\x1b[0m";
    let dim = "\x1b[2m";

    let d = match dto.as_ref() {
        Some(d) => d,
        None => {
            // Explicit unknown — never silently omit missing freshness.
            return vec![format!("{dim}Freshness:{reset} \x1b[2m[unknown]{reset}")];
        }
    };

    let mut lines = Vec::new();
    let status = classify_freshness(dto);
    let color = match status {
        "current" => "\x1b[32m",
        "changed" => "\x1b[33m",
        "unreachable" => "\x1b[31m",
        _ => "\x1b[2m",
    };

    // Status line — always shown when a DTO exists.
    lines.push(format!("{dim}Freshness:{reset} {color}[{status}]{reset}"));

    // Reason.
    if let Some(r) = d.reason.as_deref() {
        lines.push(format!("{dim}Reason:{reset} {r}"));
    }
    // Rename context.
    if let Some(r) = d.renamed_to.as_deref() {
        lines.push(format!("{dim}Renamed to:{reset} {r}"));
    }

    // Timestamps.
    if let Some(ts) = d.last_checked_at.as_deref() {
        lines.push(format!("{dim}Last checked:{reset} {ts}"));
    }
    if let Some(ts) = d.last_verified_at.as_deref() {
        lines.push(format!("{dim}Last verified:{reset} {ts}"));
    }
    if let Some(ts) = d.last_change_detected_at.as_deref() {
        lines.push(format!("{dim}Last change:{reset} {ts}"));
    }

    // Retryable — always present so a check_failed/unreachable state is
    // actionable for the user (tells them whether re-checking is possible).
    lines.push(format!(
        "{dim}Retryable:{reset} {}",
        if d.retryable { "yes" } else { "no" }
    ));

    // Hashes — full values (no abbreviation here; callers abbreviate if they
    // need to fit a compare column).
    if let Some(h) = d.scanned_hash.as_deref() {
        lines.push(format!("{dim}Scanned hash:{reset} {h}"));
    }
    if let Some(h) = d.latest_upstream_hash.as_deref() {
        lines.push(format!("{dim}Upstream hash:{reset} {h}"));
    }

    lines
}

/// Abbreviated hash for compare columns — `sha256` hashes are 64 hex chars;
/// show 8 + `…` + 8 to keep two compare columns readable.
pub fn abbrev_hash(h: &str) -> String {
    if h.len() <= 16 {
        h.to_string()
    } else {
        format!("{}…{}", &h[..8], &h[h.len() - 8..])
    }
}

/// Pair two optional freshness display cells for a symmetric compare row.
///
/// Returns `Some((left, right))` when at least one side has a value — the
/// missing side is rendered as `—`. Returns `None` only when both sides are
/// absent, so callers can skip the whole row. This makes human compare
/// symmetric: a value on the right-hand-only side still produces a row.
pub fn compare_row(left: Option<String>, right: Option<String>) -> Option<(String, String)> {
    match (left, right) {
        (None, None) => None,
        (l, r) => Some((
            l.unwrap_or_else(|| "—".to_string()),
            r.unwrap_or_else(|| "—".to_string()),
        )),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A `PublicFreshness` with every optional field populated, for tests that
    /// need the full wire shape.
    fn dto_with_all_fields() -> PublicFreshness {
        PublicFreshness {
            status: "changed".into(),
            reason: Some("upstream moved".into()),
            retryable: true,
            renamed_to: Some("org/old-name".into()),
            last_checked_at: Some("2026-06-15T12:00:00Z".into()),
            last_verified_at: Some("2026-06-15T12:01:00Z".into()),
            last_change_detected_at: Some("2026-06-14T08:30:00Z".into()),
            scanned_hash: Some("aaa".into()),
            latest_upstream_hash: Some("bbb".into()),
        }
    }

    // ── Deserialization ─────────────────────────────────────────────────

    #[test]
    fn deserialize_full_dto() {
        let json = r#"{
            "status": "verified_unchanged",
            "reason": null,
            "retryable": false,
            "renamedTo": null,
            "lastCheckedAt": "2026-07-01T00:00:00Z",
            "lastVerifiedAt": "2026-07-01T00:01:00Z",
            "lastChangeDetectedAt": null,
            "scannedHash": "abcdef0123456789",
            "latestUpstreamHash": "fedcba9876543210"
        }"#;
        let dto: PublicFreshness = serde_json::from_str(json).unwrap();
        assert_eq!(dto.status, "verified_unchanged");
        assert_eq!(dto.reason, None);
        assert!(!dto.retryable);
        assert_eq!(dto.renamed_to, None);
        assert_eq!(dto.last_checked_at, Some("2026-07-01T00:00:00Z".into()));
        assert_eq!(dto.last_verified_at, Some("2026-07-01T00:01:00Z".into()));
        assert_eq!(dto.last_change_detected_at, None);
        assert_eq!(dto.scanned_hash, Some("abcdef0123456789".into()));
        assert_eq!(dto.latest_upstream_hash, Some("fedcba9876543210".into()));
    }

    #[test]
    fn deserialize_null_freshness_is_none() {
        let json = r#"{"status": "verified_unchanged", "reason": null, "retryable": false, "renamedTo": null, "lastCheckedAt": null, "lastVerifiedAt": null, "lastChangeDetectedAt": null, "scannedHash": null, "latestUpstreamHash": null}"#;
        let dto: PublicFreshness = serde_json::from_str(json).unwrap();
        // All fields are present but null. This is a valid DTO row — the
        // absence signal is a completely missing `freshness` key on the
        // parent, which yields None on the optional field.
        assert_eq!(dto.scanned_hash, None);
    }

    #[test]
    fn serialize_round_trip() {
        let original = dto_with_all_fields();
        let json = serde_json::to_string(&original).unwrap();
        let restored: PublicFreshness = serde_json::from_str(&json).unwrap();
        // Every field must round-trip losslessly — assert all nine.
        assert_eq!(restored.status, "changed");
        assert_eq!(restored.reason, Some("upstream moved".into()));
        assert!(restored.retryable);
        assert_eq!(restored.renamed_to, Some("org/old-name".into()));
        assert_eq!(
            restored.last_checked_at,
            Some("2026-06-15T12:00:00Z".into())
        );
        assert_eq!(
            restored.last_verified_at,
            Some("2026-06-15T12:01:00Z".into())
        );
        assert_eq!(
            restored.last_change_detected_at,
            Some("2026-06-14T08:30:00Z".into())
        );
        assert_eq!(restored.scanned_hash, Some("aaa".into()));
        assert_eq!(restored.latest_upstream_hash, Some("bbb".into()));
    }

    #[test]
    fn serialize_with_optionals_none_omits_them_but_status_retryable_remain() {
        // DTO with all optional fields `None` still carries status + retryable,
        // which are required (non-optional) fields.
        let original = PublicFreshness {
            status: "unreachable".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        let val: serde_json::Value = serde_json::to_value(&original).unwrap();
        assert_eq!(val["status"], "unreachable");
        assert_eq!(val["retryable"], false);
        // When the DTO itself is present, its optional fields serialize as
        // present-but-null (the DTO row exists on the server). This contrasts
        // with an absent DTO, which the parent struct omits entirely.
        assert!(val.get("reason").is_some() && val["reason"].is_null());
        assert!(val.get("scannedHash").is_some() && val["scannedHash"].is_null());
    }

    #[test]
    fn all_four_statuses_deserialize() {
        for status in [
            "verified_unchanged",
            "changed",
            "unreachable",
            "check_failed",
        ] {
            let json = serde_json::json!({
                "status": status,
                "reason": null,
                "retryable": false,
                "renamedTo": null,
                "lastCheckedAt": null,
                "lastVerifiedAt": null,
                "lastChangeDetectedAt": null,
                "scannedHash": null,
                "latestUpstreamHash": null,
            });
            let dto: PublicFreshness = serde_json::from_value(json).unwrap();
            assert_eq!(dto.status, status);
        }
    }

    // ── Classification ──────────────────────────────────────────────────

    #[test]
    fn classify_verified_unchanged_is_current() {
        let f = PublicFreshness {
            status: "verified_unchanged".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        assert_eq!(classify_freshness(&Some(&f)), "current");
    }

    #[test]
    fn classify_changed_is_changed() {
        let f = PublicFreshness {
            status: "changed".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        assert_eq!(classify_freshness(&Some(&f)), "changed");
    }

    #[test]
    fn classify_unreachable_is_unreachable() {
        let f = PublicFreshness {
            status: "unreachable".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        assert_eq!(classify_freshness(&Some(&f)), "unreachable");
    }

    #[test]
    fn classify_check_failed_is_not_current() {
        let f = PublicFreshness {
            status: "check_failed".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        assert_ne!(classify_freshness(&Some(&f)), "current");
        assert_eq!(classify_freshness(&Some(&f)), "unknown");
    }

    #[test]
    fn null_freshness_is_not_current() {
        assert_eq!(classify_freshness(&None), "unknown");
    }

    #[test]
    fn empty_option_is_not_current() {
        let f = PublicFreshness {
            status: String::new(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        assert_eq!(classify_freshness(&Some(&f)), "unknown");
    }

    // ── Compact display ─────────────────────────────────────────────────

    #[test]
    fn compact_display_labels_distinguish_all_statuses() {
        let unchanged = PublicFreshness {
            status: "verified_unchanged".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        let changed = PublicFreshness {
            status: "changed".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        let unreachable = PublicFreshness {
            status: "unreachable".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };

        let a = fmt_freshness_compact(&Some(&unchanged));
        let b = fmt_freshness_compact(&Some(&changed));
        let c = fmt_freshness_compact(&Some(&unreachable));
        let d = fmt_freshness_compact(&None);

        // All four must be distinct — the whole point of compact output.
        assert_eq!(a, "[ok]");
        assert_eq!(b, "[chgd]");
        assert_eq!(c, "[offline]");
        assert_eq!(d, "[? ]");
        assert!(a != b && b != c && c != d && a != d);
    }

    #[test]
    fn compact_label_fits_in_narrow_column() {
        let f = PublicFreshness {
            status: "verified_unchanged".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        assert!(fmt_freshness_compact(&Some(&f)).len() <= 14);
    }

    // ── Detailed display ────────────────────────────────────────────────

    #[test]
    fn detail_shows_status_and_timestamps() {
        let f = PublicFreshness {
            status: "verified_unchanged".into(),
            reason: None,
            retryable: false,
            renamed_to: Some("org/rename".into()),
            last_checked_at: Some("2026-07-01T00:00:00Z".into()),
            last_verified_at: Some("2026-07-01T00:01:00Z".into()),
            last_change_detected_at: None,
            scanned_hash: Some("abc123".into()),
            latest_upstream_hash: Some("def456".into()),
        };
        let lines = fmt_freshness_detail(&Some(&f));
        assert!(!lines.is_empty());
        let text: String = lines.join("\n");
        assert!(text.contains("[current]"));
        assert!(text.contains("Renamed to:"));
        assert!(text.contains("2026-07-01T00:00:00Z"));
        assert!(text.contains("abc123"));
        assert!(text.contains("def456"));
    }

    #[test]
    fn detail_for_null_freshness_renders_explicit_unknown() {
        // Missing freshness must never be silently omitted in the rich view —
        // an explicit "unknown" state is shown instead (see slice requirement).
        let lines = fmt_freshness_detail(&None);
        assert!(!lines.is_empty());
        let text: String = lines.join("\n");
        assert!(text.contains("[unknown]"), "got: {text}");
        assert!(
            !text.contains("[current]"),
            "missing must not look current: {text}"
        );
    }

    #[test]
    fn detail_renders_reason_and_retryable_for_check_failed() {
        let f = PublicFreshness {
            status: "check_failed".into(),
            reason: Some("upstream timed out".into()),
            retryable: true,
            renamed_to: None,
            last_checked_at: Some("2026-07-01T00:00:00Z".into()),
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        let lines = fmt_freshness_detail(&Some(&f));
        let text: String = lines.join("\n");
        // check_failed is non-current (unknown), reason and retryable preserved.
        assert!(text.contains("[unknown]"), "got: {text}");
        assert!(
            !text.contains("[current]"),
            "check_failed must not be current: {text}"
        );
        assert!(
            text.contains("upstream timed out"),
            "reason preserved: {text}"
        );
        assert!(text.contains("Retryable:"), "retryable rendered: {text}");
    }

    #[test]
    fn detail_renders_not_retryable_when_false() {
        let f = PublicFreshness {
            status: "unreachable".into(),
            reason: None,
            retryable: false,
            renamed_to: None,
            last_checked_at: None,
            last_verified_at: None,
            last_change_detected_at: None,
            scanned_hash: None,
            latest_upstream_hash: None,
        };
        let text: String = fmt_freshness_detail(&Some(&f)).join("\n");
        // The dim/reset ANSI codes wrap "Retryable:" — assert on the dimmed
        // label plus the plain trailing value.
        assert!(text.contains("\x1b[2mRetryable:\x1b[0m no"), "got: {text}");
        assert!(!text.contains("Retryable: yes"), "got: {text}");
    }

    // ── Compare pairing ─────────────────────────────────────────────────

    #[test]
    fn compare_row_both_sides_present() {
        let row = compare_row(Some("a".into()), Some("b".into())).unwrap();
        assert_eq!(row, (String::from("a"), String::from("b")));
    }

    #[test]
    fn compare_row_right_hand_only_still_emits_row() {
        // Symmetric compare: a value on only the right-hand side still produces
        // a row, with `—` for the missing left side.
        let row = compare_row(None, Some("b-only".into())).unwrap();
        assert_eq!(row.0, "—");
        assert_eq!(row.1, "b-only");
    }

    #[test]
    fn compare_row_left_hand_only_still_emits_row() {
        let row = compare_row(Some("a-only".into()), None).unwrap();
        assert_eq!(row.0, "a-only");
        assert_eq!(row.1, "—");
    }

    #[test]
    fn compare_row_both_absent_is_none() {
        assert!(compare_row(None, None).is_none());
    }

    // ── Hash abbreviation ───────────────────────────────────────────────

    #[test]
    fn abbrev_hash_short_passthrough() {
        assert_eq!(abbrev_hash("abc"), "abc");
        assert_eq!(abbrev_hash("12345678"), "12345678");
    }

    #[test]
    fn abbrev_hash_long_truncates_both_ends() {
        let full = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        assert_eq!(abbrev_hash(full), "abcdef01…23456789");
    }

    #[test]
    fn abbrev_hash_boundary_at_16_chars() {
        let s = "1234567890123456"; // exactly 16
        assert_eq!(abbrev_hash(s), s); // not abbreviated
    }
}
