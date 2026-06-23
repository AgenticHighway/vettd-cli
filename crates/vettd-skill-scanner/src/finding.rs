//! Canonical `Finding` type and associated enums.
//!
//! These types define the vettd wire format for per-asset skill findings.
//! Serde attributes on every type are part of the public contract — do not
//! change them without a corresponding wire-format version bump.

use serde::{Deserialize, Serialize};

use crate::consts::DEFAULT_SOURCE;

fn default_source() -> String {
    DEFAULT_SOURCE.to_string()
}

// vettd's skill-analyzer never sets the `source` field on its findings, so it
// is absent (null) in the wire format. For parity we omit the field when it
// carries the default "vettd" value, matching vettd's implicit-null behavior.
fn is_default_source(s: &str) -> bool {
    s == DEFAULT_SOURCE
}

// ---------------------------------------------------------------------------
// Finding
// ---------------------------------------------------------------------------

/// A single check result produced by the skill scanner for one asset.
///
/// Maps onto `AssetFinding` in the vettd web app (`packages/types/src/asset-finding.ts`).
/// Fields that are server-computed (`id`, `skillAuditId`, `fingerprint`, `sources`, `index`)
/// are not present here — the scanner does not emit them.
///
/// **Wire format note**: The `detail` field on file-specific findings must embed the
/// source location using the format `"Detected in <filepath>:<linenum> — \`snippet\`"`.
/// Chain detection in the vettd web app parses this string with a regex; changing the
/// format is a breaking change.
///
/// **Ordering note**: `Severity` derives `Ord` as `Info < Low < Medium < High < Critical`.
/// This is the opposite of vettd's `SEVERITY_ORDER` sort indices (Critical = 0). Use
/// `Ord` for threshold comparisons; invert the ordering for display/sort operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// Rule identifier (`VTD-0001`–`VTD-0123` for built-in rules; upstream ID for
    /// external scanners). Empty string when not applicable.
    #[serde(default)]
    pub rule_id: String,

    /// Which check category this finding belongs to.
    pub category: FindingCategory,

    /// Finding severity. Chain detection may mutate this to `Critical` at scan time;
    /// grade computation must therefore run **after** chain detection completes.
    pub severity: Severity,

    /// Human-readable check name — primary display text on every UI surface.
    pub label: String,

    /// Extended description. For file-specific findings this is also a wire protocol:
    /// embed the source location as `"Detected in <filepath>:<linenum> — \`snippet\`"`.
    pub detail: String,

    /// Relative path from the asset root to the file that produced this finding.
    /// Absent for package-level findings. DB stores `""` for absent; the CLI emits
    /// the field only when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filepath: Option<String>,

    /// OWASP Top 10 for LLM Applications (2025) category, if applicable (e.g. `"LLM01"`).
    /// Must be set by the scanner via its own label-map lookup; the server stores exactly
    /// what the scanner emits.
    #[serde(default)]
    pub owasp_llm_category: Option<String>,

    /// Groups co-occurring findings that form an attack chain. Set by chain detection
    /// after the main scan pass. All findings with the same `chain_id` belong to the
    /// same chain.
    #[serde(default)]
    pub chain_id: Option<String>,

    /// Whether the pattern suggests deliberate harmful intent (`Malicious`) versus poor
    /// hygiene (`Negligent`). Set at rule-definition time and may be mutated by chain
    /// detection. Drives `hasMaliciousFindings` on the audit record.
    #[serde(default)]
    pub intent: Option<Intent>,

    /// Scanner that produced this finding. Defaults to `"vettd"` for first-party findings.
    /// Kept as `String` (not enum) to remain open to third-party scanner names.
    /// Omitted from serialized output when set to the default value so that the wire
    /// format matches vettd's behaviour of not emitting the field for first-party findings.
    #[serde(default = "default_source", skip_serializing_if = "is_default_source")]
    pub source: String,
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Which aspect of a skill this finding addresses.
///
/// Serialises to kebab-case to match the vettd wire format (`"best-practices"`).
/// `#[non_exhaustive]` allows adding categories in future wire-format revisions
/// without breaking existing deserializers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum FindingCategory {
    Security,
    Structure,
    /// Serialises as `"best-practices"`.
    BestPractices,
    Description,
    Scripts,
    Evals,
}

impl FindingCategory {
    /// Returns the wire-format string for this category.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Security => "security",
            Self::Structure => "structure",
            Self::BestPractices => "best-practices",
            Self::Description => "description",
            Self::Scripts => "scripts",
            Self::Evals => "evals",
        }
    }
}

/// Finding severity, ordered from least to most severe.
///
/// **`Ord` direction**: `Info < Low < Medium < High < Critical`. This is the
/// natural ascending order for threshold comparisons (`severity >= Severity::High`).
/// The vettd web app's `SEVERITY_ORDER` sort indices run in the **opposite** direction
/// (Critical = 0) — invert when sorting for display.
///
/// `Info` findings are excluded from all grade threshold computations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// Returns the wire-format string for this severity.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// Whether a finding represents deliberate harm or poor hygiene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    Malicious,
    Negligent,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_finding(category: FindingCategory, severity: Severity) -> Finding {
        Finding {
            rule_id: "VTD-0001".to_string(),
            category,
            severity,
            label: "Test finding".to_string(),
            detail: "Detail text".to_string(),
            filepath: None,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: default_source(),
        }
    }

    // --- Wire-format contract tests -----------------------------------------
    // These guard the serde representation against the vettd wire format.
    // If any assertion here breaks, the CLI output has diverged from what the
    // vettd server expects.

    #[test]
    fn finding_serialises_to_camel_case() {
        // The vettd server deserialises camelCase JSON keys.
        let f = Finding {
            rule_id: "VTD-0001".to_string(),
            category: FindingCategory::Security,
            severity: Severity::High,
            label: "Test".to_string(),
            detail: "Detail".to_string(),
            filepath: Some("scripts/run.sh".to_string()),
            owasp_llm_category: Some("LLM01".to_string()),
            chain_id: Some("cred-exfil-0".to_string()),
            intent: Some(Intent::Malicious),
            source: "vettd".to_string(),
        };
        let v = serde_json::to_value(&f).unwrap();
        assert!(v.get("ruleId").is_some(), "ruleId key must be camelCase");
        assert!(v.get("rule_id").is_none(), "snake_case must not appear");
        assert!(
            v.get("owaspLlmCategory").is_some(),
            "owaspLlmCategory key must be camelCase"
        );
        assert!(v.get("chainId").is_some(), "chainId key must be camelCase");
        assert!(v.get("filepath").is_some(), "filepath is present when Some");
    }

    #[test]
    fn filepath_none_is_omitted_not_null() {
        // The DB default for filepath is ""; a null/absent value in the wire
        // format should be handled by the server, not emitted as JSON null.
        let f = minimal_finding(FindingCategory::Security, Severity::Info);
        let v = serde_json::to_value(&f).unwrap();
        assert!(
            v.get("filepath").is_none(),
            "filepath must be absent (not null) when None"
        );
    }

    #[test]
    fn category_best_practices_serialises_as_kebab() {
        // vettd stores "best-practices" in the DB; "bestPractices" would be rejected.
        let f = minimal_finding(FindingCategory::BestPractices, Severity::Low);
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["category"], "best-practices");
    }

    #[test]
    fn severity_serialises_lowercase() {
        for (sev, expected) in [
            (Severity::Info, "info"),
            (Severity::Low, "low"),
            (Severity::Medium, "medium"),
            (Severity::High, "high"),
            (Severity::Critical, "critical"),
        ] {
            let f = minimal_finding(FindingCategory::Security, sev);
            let v = serde_json::to_value(&f).unwrap();
            assert_eq!(v["severity"], expected);
        }
    }

    #[test]
    fn source_defaults_to_vettd_on_missing_input() {
        // When source is absent in JSON, it must default to "vettd" so existing
        // server-side multi-scanner consensus logic can attribute the finding.
        let json = r#"{"category":"security","severity":"low","label":"x","detail":"y"}"#;
        let f: Finding = serde_json::from_str(json).unwrap();
        assert_eq!(f.source, "vettd");
    }

    #[test]
    fn source_default_matches_const() {
        assert_eq!(default_source(), crate::consts::DEFAULT_SOURCE);
    }

    #[test]
    fn rule_id_defaults_to_empty_string_on_missing_input() {
        let json = r#"{"category":"structure","severity":"info","label":"x","detail":"y"}"#;
        let f: Finding = serde_json::from_str(json).unwrap();
        assert_eq!(f.rule_id, "");
    }

    #[test]
    fn serde_round_trip() {
        let original = Finding {
            rule_id: "VTD-0042".to_string(),
            category: FindingCategory::BestPractices,
            severity: Severity::Medium,
            label: "Round-trip test".to_string(),
            detail: "Detected in scripts/run.sh:12 — `eval $INPUT`".to_string(),
            filepath: Some("scripts/run.sh".to_string()),
            owasp_llm_category: Some("LLM06".to_string()),
            chain_id: None,
            intent: Some(Intent::Negligent),
            source: "vettd".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: Finding = serde_json::from_str(&json).unwrap();
        assert_eq!(original.rule_id, restored.rule_id);
        assert_eq!(original.category, restored.category);
        assert_eq!(original.severity, restored.severity);
        assert_eq!(original.label, restored.label);
        assert_eq!(original.detail, restored.detail);
        assert_eq!(original.filepath, restored.filepath);
        assert_eq!(original.owasp_llm_category, restored.owasp_llm_category);
        assert_eq!(original.chain_id, restored.chain_id);
        assert_eq!(original.source, restored.source);
    }

    // --- Ord / threshold tests ----------------------------------------------

    #[test]
    fn severity_ord_ascending() {
        // Used for grade threshold comparisons: severity >= Severity::High
        assert!(Severity::Info < Severity::Low);
        assert!(Severity::Low < Severity::Medium);
        assert!(Severity::Medium < Severity::High);
        assert!(Severity::High < Severity::Critical);
    }

    #[test]
    fn severity_ord_threshold_example() {
        // The grade formula uses: critical >= 1 OR high >= 3 → F
        // Verify that the Ord impl makes severity-based counting straightforward.
        let findings = vec![
            minimal_finding(FindingCategory::Security, Severity::High),
            minimal_finding(FindingCategory::Security, Severity::High),
            minimal_finding(FindingCategory::Security, Severity::High),
        ];
        let high_count = findings
            .iter()
            .filter(|f| f.severity == Severity::High)
            .count();
        assert_eq!(high_count, 3); // three highs → F grade
    }

    // --- as_str helpers -----------------------------------------------------

    #[test]
    fn category_as_str_matches_serde() {
        let pairs = [
            (FindingCategory::Security, "security"),
            (FindingCategory::Structure, "structure"),
            (FindingCategory::BestPractices, "best-practices"),
            (FindingCategory::Description, "description"),
            (FindingCategory::Scripts, "scripts"),
            (FindingCategory::Evals, "evals"),
        ];
        for (cat, expected) in pairs {
            assert_eq!(cat.as_str(), expected);
            // Also verify serde agrees
            let v = serde_json::to_value(&cat).unwrap();
            assert_eq!(v.as_str().unwrap(), expected);
        }
    }

    #[test]
    fn severity_as_str_matches_serde() {
        let pairs = [
            (Severity::Info, "info"),
            (Severity::Low, "low"),
            (Severity::Medium, "medium"),
            (Severity::High, "high"),
            (Severity::Critical, "critical"),
        ];
        for (sev, expected) in pairs {
            assert_eq!(sev.as_str(), expected);
            let v = serde_json::to_value(&sev).unwrap();
            assert_eq!(v.as_str().unwrap(), expected);
        }
    }
}
