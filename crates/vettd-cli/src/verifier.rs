//! Pure-logic verification module.
//!
//! Applies governance rules to an `ArtifactReport` and sets its
//! `verification_status` to one of the five severity levels.

use crate::models::ArtifactReport;
use crate::scoring::{
    SEVERITY_CRITICAL_SCORE, SEVERITY_HIGH_SCORE, SEVERITY_LOW_SCORE, SEVERITY_MEDIUM_SCORE,
};

// ---------------------------------------------------------------------------
// Severity level constants
// ---------------------------------------------------------------------------

pub const SEVERITY_CRITICAL: &str = "critical";
pub const SEVERITY_HIGH: &str = "high";
pub const SEVERITY_MEDIUM: &str = "medium";
pub const SEVERITY_LOW: &str = "low";
pub const SEVERITY_INFO: &str = "info";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the higher-severity of `a` and `b`.
fn rank_max<'a>(a: &'a str, b: &'a str) -> &'a str {
    let rank = |s: &str| match s {
        SEVERITY_CRITICAL => 5,
        SEVERITY_HIGH => 4,
        SEVERITY_MEDIUM => 3,
        SEVERITY_LOW => 2,
        SEVERITY_INFO => 1,
        _ => 0,
    };
    if rank(a) >= rank(b) {
        a
    } else {
        b
    }
}

/// Map a numeric risk score to a severity level string.
fn score_to_severity(score: i32) -> &'static str {
    if score >= SEVERITY_CRITICAL_SCORE {
        SEVERITY_CRITICAL
    } else if score >= SEVERITY_HIGH_SCORE {
        SEVERITY_HIGH
    } else if score >= SEVERITY_MEDIUM_SCORE {
        SEVERITY_MEDIUM
    } else if score >= SEVERITY_LOW_SCORE {
        SEVERITY_LOW
    } else {
        SEVERITY_INFO
    }
}

/// True when the artifact declares tools, permissions, or API endpoints.
fn has_governance_constraints(artifact: &ArtifactReport) -> bool {
    let meta = &artifact.metadata;
    for key in &["declared_tools", "permissions", "api_endpoints"] {
        if let Some(v) = meta.get(*key) {
            if let Some(arr) = v.as_array() {
                if !arr.is_empty() {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Apply verification rules and return the resulting severity string.
///
/// Rules (in priority order):
///   1. `credential_exposure_signal` → always **critical**.
///   2. Score bands: ≥90 → critical, ≥70 → high, ≥40 → medium, ≥10 → low, else info.
///   3. `dangerous_keyword:*` escalates to high unless governance
///      constraints are present (then medium).
///   4. `dangerous_combo:*` escalates to at least medium.
pub fn verify(artifact: &mut ArtifactReport) -> String {
    // Rule 1: credential exposure is an automatic critical.
    if artifact
        .signals
        .contains(&"credential_exposure_signal".to_string())
    {
        artifact.verification_status = SEVERITY_CRITICAL.to_string();
        return artifact.verification_status.clone();
    }

    // Rule 2: score-based bands.
    let mut status = score_to_severity(artifact.risk_score);

    // Rule 3-4: dangerous signal escalation.
    let has_dangerous_keyword = artifact
        .signals
        .iter()
        .any(|s| s.starts_with("dangerous_keyword:"));
    let has_dangerous_combo = artifact
        .signals
        .iter()
        .any(|s| s.starts_with("dangerous_combo:"));

    if has_dangerous_keyword && status != SEVERITY_CRITICAL {
        status = if has_governance_constraints(artifact) {
            rank_max(status, SEVERITY_MEDIUM)
        } else {
            rank_max(status, SEVERITY_HIGH)
        };
    } else if has_dangerous_combo && status != SEVERITY_CRITICAL {
        status = rank_max(status, SEVERITY_MEDIUM);
    }

    artifact.verification_status = status.to_string();
    artifact.verification_status.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artifact_with(atype: &str, score: i32, signals: &[&str]) -> ArtifactReport {
        let mut a = ArtifactReport::new(atype, 0.8);
        a.risk_score = score;
        a.signals = signals.iter().map(|s| s.to_string()).collect();
        a
    }

    #[test]
    fn credential_exposure_always_critical() {
        let mut a = artifact_with("mcp_config", 5, &["credential_exposure_signal"]);
        let status = verify(&mut a);
        assert_eq!(status, SEVERITY_CRITICAL);
    }

    #[test]
    fn score_90_is_critical() {
        let mut a = artifact_with("cursor_rules", 90, &[]);
        assert_eq!(verify(&mut a), SEVERITY_CRITICAL);
    }

    #[test]
    fn score_70_is_high() {
        let mut a = artifact_with("cursor_rules", 70, &[]);
        assert_eq!(verify(&mut a), SEVERITY_HIGH);
    }

    #[test]
    fn score_40_is_medium() {
        let mut a = artifact_with("cursor_rules", 40, &[]);
        assert_eq!(verify(&mut a), SEVERITY_MEDIUM);
    }

    #[test]
    fn score_10_is_low() {
        let mut a = artifact_with("cursor_rules", 10, &[]);
        assert_eq!(verify(&mut a), SEVERITY_LOW);
    }

    #[test]
    fn score_below_10_is_info() {
        let mut a = artifact_with("cursor_rules", 5, &[]);
        assert_eq!(verify(&mut a), SEVERITY_INFO);
    }

    #[test]
    fn dangerous_keyword_without_governance_escalates_to_high() {
        let mut a = artifact_with("cursor_rules", 5, &["dangerous_keyword:steal"]);
        assert_eq!(verify(&mut a), SEVERITY_HIGH);
    }

    #[test]
    fn dangerous_keyword_with_governance_escalates_to_medium() {
        let mut a = artifact_with("cursor_rules", 5, &["dangerous_keyword:steal"]);
        a.metadata
            .insert("declared_tools".to_string(), json!(["shell"]));
        assert_eq!(verify(&mut a), SEVERITY_MEDIUM);
    }

    #[test]
    fn dangerous_combo_escalates_to_medium() {
        let mut a = artifact_with("cursor_rules", 5, &["dangerous_combo:shell+network+fs"]);
        assert_eq!(verify(&mut a), SEVERITY_MEDIUM);
    }

    #[test]
    fn credential_overrides_low_score() {
        let mut a = artifact_with("cursor_rules", 0, &["credential_exposure_signal"]);
        assert_eq!(verify(&mut a), SEVERITY_CRITICAL);
    }

    #[test]
    fn high_score_with_dangerous_keyword_stays_at_score_band() {
        // score already at high, dangerous_keyword without governance → floor at high → no change
        let mut a = artifact_with("cursor_rules", 75, &["dangerous_keyword:steal"]);
        assert_eq!(verify(&mut a), SEVERITY_HIGH);
    }

    #[test]
    fn critical_score_not_downgraded_by_governance() {
        let mut a = artifact_with("cursor_rules", 95, &["dangerous_keyword:steal"]);
        a.metadata
            .insert("declared_tools".to_string(), json!(["shell"]));
        assert_eq!(verify(&mut a), SEVERITY_CRITICAL);
    }

    #[test]
    fn has_governance_constraints_checks_declared_tools() {
        let mut a = ArtifactReport::new("test", 0.5);
        assert!(!has_governance_constraints(&a));
        a.metadata.insert("declared_tools".into(), json!(["bash"]));
        assert!(has_governance_constraints(&a));
    }

    #[test]
    fn has_governance_constraints_checks_permissions() {
        let mut a = ArtifactReport::new("test", 0.5);
        a.metadata.insert("permissions".into(), json!(["read"]));
        assert!(has_governance_constraints(&a));
    }

    #[test]
    fn has_governance_empty_arrays_not_constraints() {
        let mut a = ArtifactReport::new("test", 0.5);
        a.metadata.insert("declared_tools".into(), json!([]));
        assert!(!has_governance_constraints(&a));
    }

    #[test]
    fn rank_max_returns_higher_severity() {
        assert_eq!(
            rank_max(SEVERITY_INFO, SEVERITY_CRITICAL),
            SEVERITY_CRITICAL
        );
        assert_eq!(
            rank_max(SEVERITY_CRITICAL, SEVERITY_INFO),
            SEVERITY_CRITICAL
        );
        assert_eq!(rank_max(SEVERITY_LOW, SEVERITY_MEDIUM), SEVERITY_MEDIUM);
        assert_eq!(rank_max(SEVERITY_HIGH, SEVERITY_HIGH), SEVERITY_HIGH);
    }
}
