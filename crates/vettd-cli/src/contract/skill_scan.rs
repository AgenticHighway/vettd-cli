//! Adapter between `vettd-skill-scanner` and the v2 contract types.
//!
//! Loads the skill's files from disk, calls `scan_skill`, and maps the result
//! onto `ExternalScannerResult` for inclusion in the contract payload.
//!
//! ## Stub note
//!
//! File loading here does a local directory re-walk. The real implementation
//! should thread the file map already assembled during discovery through to
//! this call instead of re-reading from disk.

use std::path::Path;

use crate::contract::helpers::first_path;
use crate::contract::identity::{self, SkillFileLoad};
use crate::contract::types::{
    ExternalScannerFinding, ExternalScannerResult, ScannerCoverage, ScannerSignal,
};
use crate::models::ArtifactReport;

use vettd_skill_scanner::consts::{CURRENT_SCANNER_VERSION, DEFAULT_SOURCE};
use vettd_skill_scanner::scan_skill;

/// Structural facts computed by the skill scanner, surfaced at the skill level
/// (`skills[].hasSkillMd`, `skills[].fileCount`, ...) rather than inside
/// `externalScannerResults[]` — see scanner-field-gate.json.
#[derive(Debug, Clone)]
pub(crate) struct SkillStructuralFacts {
    pub file_count: usize,
    pub has_skill_md: bool,
    pub has_scripts: bool,
    pub has_references: bool,
    pub has_evals: bool,
    pub has_assets: bool,
}

/// Content identity derived from the same directory load that produced the
/// scan, so an assessment can never be paired with a digest of other bytes
/// (#315/P2-10).
#[derive(Debug, Clone)]
pub(crate) struct SkillIdentity {
    /// Digest over the analyzed text files (hub `contentHash` parity).
    pub content_hash: String,
    /// Digest over every byte under the root; `None` when any entry could not
    /// be read, was too large, or the walk hit its budget.
    pub bundle_digest: Option<String>,
    /// Paths dropped from identity by disclosure rules.
    pub exclusions: Vec<String>,
    /// True only when the load covered the whole bundle and produced a digest,
    /// so the bundle digest is safe to use as an identity key.
    pub complete: bool,
    /// Fingerprint of the loader state that shapes the assessment — entry kinds,
    /// exec bits and the shortfall reasons. Content hashes are mode-independent
    /// by agreement, so a cached assessment is only valid when this matches too
    /// (#314/N1).
    pub assessment_fingerprint: String,
}

/// Full output of one skill scan: the contract-shaped `ExternalScannerResult`
/// plus the raw structural facts the skill builder surfaces at the skill level.
#[derive(Debug, Clone)]
pub(crate) struct SkillScanOutput {
    pub external: ExternalScannerResult,
    /// `None` when the scanner errored: facts it never computed are not
    /// fabricated as `false`/`0`.
    pub structural: Option<SkillStructuralFacts>,
    /// Stable reason codes for analysis that did not cover the whole skill
    /// (#314). Empty means the scanner saw every file it should have.
    pub incomplete_reasons: Vec<String>,
    /// True when the directory load itself fell short (walk cap, unreadable
    /// entry/directory, size cap). Distinct from `incomplete_reasons` so a
    /// loader shortfall can never be mistaken for a complete analysis.
    pub partial: bool,
    /// Identity from the same load as this scan (#315/P2-10).
    pub identity: SkillIdentity,
    /// The `SKILL.md` text this scan analyzed, so the skill-level frontmatter
    /// (description/version/license/declared name) is read from the same bytes
    /// as `contentHash` and `bundleDigest` instead of a later, separate read
    /// (#314/P2).
    pub skill_md_text: Option<String>,
}

/// Whether analysis of one skill covered the whole skill (#314).
///
/// Anything but `complete` must never yield an affirmative trust decision: a
/// reader has to be able to tell "analyzed, nothing found" from "not analyzed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assessment {
    /// `complete` | `partial` | `failed` (`unsupported` is reserved in the
    /// contract and not emitted by this build).
    pub state: &'static str,
    pub reasons: Vec<String>,
}

impl Assessment {
    pub fn is_complete(&self) -> bool {
        self.state == "complete"
    }
}

/// Derive the assessment state of a skill from its scanner output. A missing
/// output or a run that did not succeed is `failed`; a loader shortfall or any
/// recorded reason is `partial`. Any state but `complete` carries at least one
/// reason, so "not analyzed" is never a silent "nothing found".
pub(crate) fn assess(output: Option<&SkillScanOutput>) -> Assessment {
    let Some(output) = output else {
        return Assessment {
            state: "failed",
            reasons: vec!["missing-result".to_string()],
        };
    };
    let mut reasons = output.incomplete_reasons.clone();
    if output.partial && reasons.is_empty() {
        // The loader always names why it fell short; this keeps the invariant
        // (partial ⇒ reasons) even if a future shortfall forgets to.
        reasons.push("file-cap".to_string());
    }
    let state = if output.external.status != "success" {
        "failed"
    } else if output.partial || !reasons.is_empty() {
        "partial"
    } else {
        "complete"
    };
    Assessment { state, reasons }
}

/// The scan output for an artifact: the cached result only when it still
/// describes the bytes on disk, otherwise a fresh scan. This is what makes a
/// changed bundle incapable of reusing a cached complete assessment
/// (#314/P2-10).
pub(crate) fn output_for(artifact: &ArtifactReport) -> Option<SkillScanOutput> {
    if let Some(cached) = &artifact.cached_skill_scan {
        if cached.matches_disk(artifact) {
            return Some(cached.clone());
        }
    }
    run_skill_scanner(artifact)
}

impl SkillScanOutput {
    /// Whether this output's snapshot still matches the directory on disk.
    /// Only a full bundle digest can prove that; without one the cache is
    /// discarded rather than risk pairing an assessment with other bytes.
    fn matches_disk(&self, artifact: &ArtifactReport) -> bool {
        let Some(cached_digest) = self.identity.bundle_digest.as_deref() else {
            return false;
        };
        let Some(root) = skill_root(artifact) else {
            return false;
        };
        let load = identity::load_skill_files(root);
        load.bundle_digest.as_deref() == Some(cached_digest)
            && identity::canonical_digest(&load.text_files) == self.identity.content_hash
            && load.assessment_fingerprint == self.identity.assessment_fingerprint
    }
}

/// Directory containing the artifact's `SKILL.md`.
fn skill_root(artifact: &ArtifactReport) -> Option<&Path> {
    let path = first_path(artifact);
    if path == "unknown" {
        return None;
    }
    Some(Path::new(path).parent().unwrap_or(Path::new(".")))
}

/// Run the skill scanner against the artifact's source directory and return
/// the full scan output (contract result + structural facts) for inclusion in
/// the contract payload.
///
/// Returns `None` if the artifact has no resolvable path or the source
/// directory cannot be located.
pub(crate) fn run_skill_scanner(artifact: &ArtifactReport) -> Option<SkillScanOutput> {
    // The pinned scanner requires a caller-supplied RFC 3339 observation time;
    // signals carry it unmodified. The pure scanner never reads a clock, so the
    // CLI stamps "now" here.
    let observed_at = chrono::Utc::now().to_rfc3339();
    run_skill_scanner_with_observed_at(artifact, &observed_at)
}

/// [`run_skill_scanner`] with an explicit observation time. A malformed value
/// is a real `scan_skill` error, which the error path below must disclose.
fn run_skill_scanner_with_observed_at(
    artifact: &ArtifactReport,
    observed_at: &str,
) -> Option<SkillScanOutput> {
    let skill_md_path = first_path(artifact);
    let skill_root = skill_root(artifact)?;

    let load = identity::load_skill_files(skill_root);
    let SkillFileLoad {
        text_files,
        all_paths,
        skipped,
        excluded,
        partial,
        incomplete_reasons,
        bundle_digest,
        assessment_fingerprint,
    } = load;
    let identity = SkillIdentity {
        content_hash: identity::canonical_digest(&text_files),
        complete: !partial && skipped.is_empty() && bundle_digest.is_some(),
        bundle_digest,
        exclusions: excluded,
        assessment_fingerprint,
    };
    let mut incomplete_reasons: Vec<String> =
        incomplete_reasons.iter().map(|r| r.to_string()).collect();
    // Captured before the scan consumes the file map: these are the exact bytes
    // the scan and the content hash describe (#314/P2).
    let skill_md_text = text_files.get("SKILL.md").cloned();
    let scan_result = match scan_skill(&text_files, &all_paths, observed_at) {
        Ok(result) => result,
        Err(e) => {
            // Report the failed run instead of dropping it: a dropped result
            // used to grade as a clean A/Trusted (#314).
            eprintln!("Warning: skill scanner error for {skill_md_path}: {e}");
            incomplete_reasons.insert(0, "scanner-error".to_string());
            return Some(SkillScanOutput {
                external: ExternalScannerResult {
                    source: "vettd".to_string(),
                    version: Some(CURRENT_SCANNER_VERSION.to_string()),
                    status: "errored".to_string(),
                    verdict: None,
                    raw_report: None,
                    findings: None,
                    signals: None,
                    coverage: None,
                },
                structural: None,
                incomplete_reasons,
                partial,
                identity,
                skill_md_text,
            });
        }
    };

    let findings: Vec<ExternalScannerFinding> =
        scan_result.findings.iter().map(map_finding).collect();

    // Signals are display-only — they travel separately from findings and are
    // never mapped into `ExternalScannerFinding` nor into the local grade.
    let signals: Vec<ScannerSignal> = scan_result
        .signals
        .iter()
        .map(|s| ScannerSignal {
            data_category: s.data_category.clone(),
            source_class: s.source_class.clone(),
            rule_id: s.rule_id.clone(),
            observed_at: s.observed_at.clone(),
            source: (s.source != DEFAULT_SOURCE).then(|| s.source.clone()),
            subject_type: s.subject_type.clone(),
            subject_id: s.subject_id.clone(),
            related_type: s.related_type.clone(),
            related_id: s.related_id.clone(),
            severity: s.severity.clone(),
            label: s.label.clone(),
            detail: s.detail.clone(),
            value_num: s.value_num,
            value_text: s.value_text.clone(),
            unit: s.unit.clone(),
            method: s.method.clone(),
            derivation: s.derivation.clone(),
            confidence: s.confidence,
            sample_size: s.sample_size,
            synthetic: s.synthetic,
            payload: s.payload.clone(),
        })
        .collect();

    let mut coverage: Vec<ScannerCoverage> = scan_result
        .coverage
        .iter()
        .map(|c| ScannerCoverage {
            kind: c.kind.clone(),
            rule_id: c.rule_id.clone(),
            label: c.label.clone(),
            detail: c.detail.clone(),
            category: c.category.clone(),
        })
        .collect();

    // Honest partial-scan disclosure (#204): files the loader could not decode
    // and caps that cut the scan short must appear in the output instead of
    // leaving a silent "success".
    if !skipped.is_empty() {
        coverage.push(ScannerCoverage {
            kind: "skipped".to_string(),
            rule_id: "scan/file-load".to_string(),
            label: "Files skipped (could not be read as text)".to_string(),
            detail: skipped.join(", "),
            category: Some("structure".to_string()),
        });
    }
    if partial {
        coverage.push(ScannerCoverage {
            kind: "partial".to_string(),
            rule_id: "scan/size-limit".to_string(),
            label: "Skill scan partial".to_string(),
            detail: format!(
                "analysis walk stopped at the {} entry budget or at the {} text-file / {} MB caps",
                identity::WALK_SAFETY_LIMIT,
                identity::MAX_SKILL_FILES,
                identity::MAX_SKILL_TOTAL_BYTES / (1024 * 1024)
            ),
            category: Some("structure".to_string()),
        });
    }

    Some(SkillScanOutput {
        external: ExternalScannerResult {
            source: "vettd".to_string(),
            version: Some(CURRENT_SCANNER_VERSION.to_string()),
            status: "success".to_string(),
            verdict: None,
            raw_report: None,
            findings: if findings.is_empty() {
                None
            } else {
                Some(findings)
            },
            signals: if signals.is_empty() {
                None
            } else {
                Some(signals)
            },
            coverage: if coverage.is_empty() {
                None
            } else {
                Some(coverage)
            },
        },
        structural: Some(SkillStructuralFacts {
            file_count: scan_result.file_count,
            has_skill_md: scan_result.has_skill_md,
            has_scripts: scan_result.has_scripts,
            has_references: scan_result.has_references,
            has_evals: scan_result.has_evals,
            has_assets: scan_result.has_assets,
        }),
        incomplete_reasons,
        partial,
        identity,
        skill_md_text,
    })
}

/// Map one scanner [`vettd_skill_scanner::Finding`] onto the contract
/// [`ExternalScannerFinding`] shape.
///
/// Pure (no I/O) so the mapping is unit-testable without a real scan run.
fn map_finding(f: &vettd_skill_scanner::Finding) -> ExternalScannerFinding {
    ExternalScannerFinding {
        rule_id: f.rule_id.clone(),
        category: f.category.as_str().to_string(),
        severity: f.severity.as_str().to_string(),
        label: f.label.clone(),
        detail: if f.detail.is_empty() {
            None
        } else {
            Some(f.detail.clone())
        },
        filepath: f.filepath.clone(),
        intent: f.intent.as_ref().map(|intent| {
            match intent {
                vettd_skill_scanner::Intent::Malicious => "malicious",
                vettd_skill_scanner::Intent::Negligent => "negligent",
            }
            .to_string()
        }),
        chain_id: f.chain_id.clone(),
        owasp_llm_category: f.owasp_llm_category.clone(),
        source: (f.source != DEFAULT_SOURCE).then(|| f.source.clone()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ArtifactReport;

    fn skill_artifact_with_path(path: &str) -> ArtifactReport {
        let mut a = ArtifactReport::new("skill", 0.9);
        a.metadata
            .insert("paths".to_string(), serde_json::json!([path]));
        a
    }

    #[test]
    fn unknown_path_returns_none() {
        // An artifact with no resolvable path must not produce a scanner result.
        let a = ArtifactReport::new("skill", 0.9);
        assert!(run_skill_scanner(&a).is_none());
    }

    #[test]
    fn nonexistent_path_returns_some_result() {
        // A path that doesn't exist on disk should still succeed — the file map
        // will just be empty and the scanner returns stub findings for a missing
        // SKILL.md.
        let a = skill_artifact_with_path("/nonexistent/path/SKILL.md");
        let result = run_skill_scanner(&a);
        assert!(result.is_some());
        let r = result.unwrap().external;
        assert_eq!(r.source, "vettd");
        assert_eq!(r.status, "success");
        assert_eq!(r.version, Some(CURRENT_SCANNER_VERSION.to_string()));
    }

    #[test]
    fn result_has_nonempty_findings_for_any_skill() {
        // Even with a nonexistent path, the scanner emits at least the
        // missing-SKILL.md finding so the contract field is populated.
        let a = skill_artifact_with_path("/nonexistent/SKILL.md");
        let result = run_skill_scanner(&a).unwrap();
        assert!(
            result
                .external
                .findings
                .as_ref()
                .is_some_and(|f| !f.is_empty()),
            "findings must be non-empty"
        );
    }

    #[test]
    fn finding_mapping_preserves_category_and_severity_strings() {
        // Verify the category/severity strings in ExternalScannerFinding match
        // the vettd wire format (lowercase, kebab-case for best-practices).
        let a = skill_artifact_with_path("/nonexistent/SKILL.md");
        let result = run_skill_scanner(&a).unwrap();
        let findings = result.external.findings.unwrap();
        for f in &findings {
            // Category must be one of the known wire values
            assert!(
                matches!(
                    f.category.as_str(),
                    "security"
                        | "structure"
                        | "best-practices"
                        | "description"
                        | "scripts"
                        | "evals"
                ),
                "unexpected category string: {}",
                f.category
            );
            // Severity must be one of the known wire values
            assert!(
                matches!(
                    f.severity.as_str(),
                    "info" | "low" | "medium" | "high" | "critical"
                ),
                "unexpected severity string: {}",
                f.severity
            );
        }
    }

    #[test]
    fn output_carries_structural_facts_at_skill_level() {
        // The six structural facts must travel with the scan output so the
        // skill builder can surface them at `skills[].<field>` (v2.6.0). A
        // nonexistent path yields the missing-SKILL.md stub, so the facts are
        // all "absent" — but they must still be present, not swallowed.
        let a = skill_artifact_with_path("/nonexistent/SKILL.md");
        let output = run_skill_scanner(&a).unwrap();
        let structural = output.structural.unwrap();
        assert_eq!(structural.file_count, 0);
        assert!(!structural.has_skill_md);
        assert!(!structural.has_scripts);
        assert!(!structural.has_references);
        assert!(!structural.has_evals);
        assert!(!structural.has_assets);
    }

    // ── signals / coverage wire-shape tests ─────────────────────────────

    fn minimal_signal() -> ScannerSignal {
        ScannerSignal {
            data_category: "characteristics".to_string(),
            source_class: "scan".to_string(),
            rule_id: "characteristics/declared-license".to_string(),
            observed_at: "2026-08-24T00:00:00Z".to_string(),
            source: None,
            subject_type: None,
            subject_id: None,
            related_type: None,
            related_id: None,
            severity: None,
            label: None,
            detail: None,
            value_num: None,
            value_text: None,
            unit: None,
            method: None,
            derivation: None,
            confidence: None,
            sample_size: None,
            synthetic: false,
            payload: None,
        }
    }

    #[test]
    fn minimal_signal_serialises_to_four_camel_case_keys() {
        // A signal with only the required fields must serialize to exactly the
        // four required camelCase keys — optional fields are omitted, never
        // emitted as null (matches the scanner's own wire contract).
        let v = serde_json::to_value(minimal_signal()).unwrap();
        let obj = v.as_object().unwrap();
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["dataCategory", "observedAt", "ruleId", "sourceClass"]
        );
    }

    #[test]
    fn signal_serialises_full_shape_in_camel_case() {
        let s = ScannerSignal {
            source: Some("third-party".to_string()),
            subject_type: Some("skill_audit".to_string()),
            subject_id: Some("audit-1".to_string()),
            related_type: Some("skill".to_string()),
            related_id: Some("other".to_string()),
            severity: Some("low".to_string()),
            label: Some("Declared license".to_string()),
            detail: Some("MIT".to_string()),
            value_num: Some(3.5),
            value_text: Some("text".to_string()),
            unit: Some("MB".to_string()),
            method: Some("parse".to_string()),
            derivation: Some("derived".to_string()),
            confidence: Some(0.9),
            sample_size: Some(12),
            synthetic: true,
            payload: Some(serde_json::Map::new()),
            ..minimal_signal()
        };
        let v = serde_json::to_value(&s).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj["sourceClass"], "scan");
        assert_eq!(obj["valueNum"], 3.5);
        assert_eq!(obj["sampleSize"], 12);
        assert_eq!(obj["synthetic"], true);
        assert_eq!(obj["relatedType"], "skill");
        assert_eq!(obj["relatedId"], "other");
        assert!(
            obj.keys().all(|k| !k.contains('_')),
            "snake_case keys must not appear: {obj:?}"
        );
    }

    #[test]
    fn signal_source_is_optional_and_default_omitted() {
        // v2.8.0: the signals item schema gains a `source` property (inserted
        // right after `sourceClass`). A populated non-default source must
        // serialize so the contract shape accepts it; the adapter folds the
        // default source ("vettd", `DEFAULT_SOURCE`) to `None`, which must
        // keep serializing to no key.
        let non_default = ScannerSignal {
            source: Some("third-party".to_string()),
            ..minimal_signal()
        };
        let v = serde_json::to_value(&non_default).unwrap();
        assert_eq!(
            v["source"], "third-party",
            "a non-default source must serialize into the contract payload"
        );

        let default_sourced = ScannerSignal {
            source: None, // what the adapter's `(s.source != DEFAULT_SOURCE)` guard produces
            ..minimal_signal()
        };
        let dv = serde_json::to_value(&default_sourced).unwrap();
        assert!(
            dv.get("source").is_none(),
            "a default-sourced signal must not carry a source key"
        );
    }

    #[test]
    fn coverage_serialises_camel_case_with_optional_category() {
        let c = ScannerCoverage {
            kind: "applicable".to_string(),
            rule_id: "VTD-0001".to_string(),
            label: "Checked".to_string(),
            detail: "Rule ran".to_string(),
            category: Some("structure".to_string()),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["kind"], "applicable");
        assert_eq!(v["category"], "structure");
        assert!(v.get("ruleId").is_some());
    }

    #[test]
    fn signals_and_coverage_do_not_become_findings() {
        // The write path must keep signals/coverage in their own arrays —
        // never folded into `findings`, which is the only array the local
        // grade reads.
        let mut result = ExternalScannerResult {
            source: "vettd".to_string(),
            version: None,
            status: "success".to_string(),
            verdict: None,
            raw_report: None,
            findings: None,
            signals: Some(vec![minimal_signal()]),
            coverage: Some(vec![ScannerCoverage {
                kind: "applicable".to_string(),
                rule_id: "VTD-0001".to_string(),
                label: "l".to_string(),
                detail: "d".to_string(),
                category: None,
            }]),
        };
        assert!(result.findings.is_none());
        assert_eq!(result.signals.as_ref().unwrap().len(), 1);
        assert_eq!(result.coverage.as_ref().unwrap().len(), 1);

        let v = serde_json::to_value(&result).unwrap();
        assert!(v.get("signals").is_some());
        assert!(v.get("coverage").is_some());
        assert!(v.get("findings").is_none());

        // Round-trip: deserialize back and confirm separation holds.
        result = serde_json::from_value(v).unwrap();
        assert!(result.findings.is_none());
        assert_eq!(
            result.signals.as_ref().unwrap()[0].rule_id,
            "characteristics/declared-license"
        );
    }

    // ── finding filepath forwarding (v2.7.0) ─────────────────────────

    fn scanner_finding(filepath: Option<String>) -> vettd_skill_scanner::Finding {
        use vettd_skill_scanner::{FindingCategory, Severity};
        vettd_skill_scanner::Finding {
            rule_id: "VTD-0001".to_string(),
            category: FindingCategory::Structure,
            severity: Severity::Info,
            label: "Test finding".to_string(),
            detail: "Detail text".to_string(),
            filepath,
            owasp_llm_category: None,
            chain_id: None,
            intent: None,
            source: "vettd".to_string(),
        }
    }

    fn contract_finding(filepath: Option<String>) -> ExternalScannerFinding {
        ExternalScannerFinding {
            rule_id: "VTD-0001".to_string(),
            category: "structure".to_string(),
            severity: "info".to_string(),
            label: "Test finding".to_string(),
            detail: Some("Detail text".to_string()),
            filepath,
            intent: None,
            chain_id: None,
            owasp_llm_category: None,
            source: None,
        }
    }

    #[test]
    fn finding_filepath_is_forwarded_into_contract_finding() {
        // CLI-ingested findings must carry the scanner's `filepath` so they
        // land in the DB with the same file attribution as GitHub/zip ingest.
        // A file-scoped finding keeps its path; a package-level finding (no
        // filepath) maps to `None`, never an empty string.
        let with = map_finding(&scanner_finding(Some("SKILL.md".to_string())));
        assert_eq!(with.filepath.as_deref(), Some("SKILL.md"));

        let without = map_finding(&scanner_finding(None));
        assert_eq!(without.filepath, None);
    }

    #[test]
    fn chain_escalation_context_survives_the_contract_boundary() {
        // #318: chain detection raises severity and sets intent + chainId.
        // Dropping them left a critical finding with no explanation in the
        // CLI record. Every decision-relevant scanner field must survive.
        let mut f = scanner_finding(Some("scripts/run.sh".to_string()));
        f.severity = vettd_skill_scanner::Severity::Critical;
        f.intent = Some(vettd_skill_scanner::Intent::Malicious);
        f.chain_id = Some("chain-1".to_string());
        f.owasp_llm_category = Some("LLM06".to_string());
        f.source = "third-party".to_string();
        let v = serde_json::to_value(map_finding(&f)).unwrap();
        assert_eq!(v["severity"], "critical");
        assert_eq!(v["intent"], "malicious");
        assert_eq!(v["chainId"], "chain-1");
        assert_eq!(v["owaspLlmCategory"], "LLM06");
        assert_eq!(v["source"], "third-party");

        // Absent context and the default source stay off the wire (additive).
        let plain = serde_json::to_value(map_finding(&scanner_finding(None))).unwrap();
        for key in ["intent", "chainId", "owaspLlmCategory", "source"] {
            assert!(plain.get(key).is_none(), "{key} must be omitted: {plain}");
        }
    }

    #[test]
    fn finding_filepath_serde_omitted_when_none_present_when_some() {
        // `filepath` is optional and additive on the wire: absent findings
        // must NOT emit `filepath: null` (keeps pre-2.7.0 payloads
        // byte-identical), present findings must emit the value.
        let none = contract_finding(None);
        let v = serde_json::to_value(&none).unwrap();
        assert!(
            v.get("filepath").is_none(),
            "absent filepath must be omitted, not null: {v}"
        );
        assert_eq!(v["ruleId"], "VTD-0001");

        let some = contract_finding(Some("SKILL.md".to_string()));
        let v = serde_json::to_value(&some).unwrap();
        assert_eq!(v["filepath"], "SKILL.md");

        // Round-trip: a finding carrying a filepath survives serialize →
        // deserialize → serialize without losing the value.
        let round: ExternalScannerFinding = serde_json::from_value(v).unwrap();
        assert_eq!(round.filepath.as_deref(), Some("SKILL.md"));
        let again = serde_json::to_value(&round).unwrap();
        assert_eq!(again["filepath"], "SKILL.md");
    }

    // ── honest scan surface (#204) ─────────────────────────────────────

    fn temp_skill_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vettd-skillscan-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn content_past_8kb_is_scanned_not_truncated() {
        // Why this matters: the old loader truncated every file at 8192 chars,
        // so content hidden past 8KB was invisible to every check while the
        // scan still reported success. An external URL that appears ONLY past
        // the old cap must now be seen by the scanner's URL rule (VTD-0088) —
        // proof the full file reached the analyzer.
        let dir = temp_skill_dir("past8k");
        let mut body = String::from("---\nname: big\ndescription: big skill\n---\n\n# Big\n\n");
        body.push_str(&"safe filler text. ".repeat(600)); // ~10.2 KB before payload
        body.push_str("See https://evil.example/payload for details.\n");
        std::fs::write(dir.join("SKILL.md"), &body).unwrap();

        let a = skill_artifact_with_path(dir.join("SKILL.md").to_string_lossy().as_ref());
        let output = run_skill_scanner(&a).unwrap();
        let findings = output.external.findings.unwrap_or_default();
        assert!(
            findings.iter().any(|f| f.rule_id == "VTD-0088"
                && f.detail
                    .as_deref()
                    .map(|d| d.contains("evil.example"))
                    .unwrap_or(false))
                || findings.iter().any(|f| f.rule_id == "VTD-0088"),
            "URL past the old 8KB cap must be found: {findings:?}"
        );
        assert!(
            output
                .external
                .coverage
                .as_ref()
                .map(|c| c.iter().all(|e| e.kind != "partial"))
                .unwrap_or(true),
            "a 10KB skill is under the caps and must not be flagged partial"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn non_utf8_file_surfaces_a_skipped_coverage_entry() {
        // A file with invalid UTF-8 used to be counted in all_paths but never
        // scanned, with no finding and no warning. It must now be disclosed.
        let dir = temp_skill_dir("nonutf8");
        std::fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
        std::fs::write(dir.join("payload.sh"), [0xff_u8, 0xfe, 0x00, 0x80]).unwrap();

        let a = skill_artifact_with_path(dir.join("SKILL.md").to_string_lossy().as_ref());
        let output = run_skill_scanner(&a).unwrap();
        let coverage = output.external.coverage.unwrap_or_default();
        let skipped = coverage
            .iter()
            .find(|c| c.kind == "skipped" && c.rule_id == "scan/file-load");
        assert!(
            skipped.is_some_and(|c| c.detail.contains("payload.sh")),
            "unreadable file must be named in coverage: {coverage:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn oversized_skill_surfaces_a_partial_coverage_entry() {
        // Hitting the 250-file cap used to silently drop files in
        // read_dir-order. The scan must now say it was partial.
        let dir = temp_skill_dir("oversized");
        std::fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
        for i in 0..300 {
            std::fs::write(dir.join(format!("f{i:03}.md")), format!("file {i}")).unwrap();
        }

        let a = skill_artifact_with_path(dir.join("SKILL.md").to_string_lossy().as_ref());
        let output = run_skill_scanner(&a).unwrap();
        let coverage = output.external.coverage.unwrap_or_default();
        assert!(
            coverage
                .iter()
                .any(|c| c.kind == "partial" && c.rule_id == "scan/size-limit"),
            "cap hit must be disclosed as partial: {coverage:?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scanner_error_is_disclosed_without_fabricated_structural_facts() {
        // Why this matters: a failed scan used to be dropped, which graded the
        // skill A/Trusted. A real `scan_skill` error must now produce an
        // explicit errored result, the `scanner-error` reason, and no
        // structural facts it never computed.
        let dir = temp_skill_dir("scanner-error");
        std::fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
        let a = skill_artifact_with_path(dir.join("SKILL.md").to_string_lossy().as_ref());

        // `scan_skill` rejects a malformed observation time; this is the real
        // error path, not a stubbed one.
        let output = run_skill_scanner_with_observed_at(&a, "not-a-timestamp").unwrap();
        assert_eq!(output.external.status, "errored");
        assert_eq!(output.external.source, "vettd");
        assert!(
            output
                .incomplete_reasons
                .contains(&"scanner-error".to_string()),
            "{:?}",
            output.incomplete_reasons
        );
        assert!(
            output.structural.is_none(),
            "no fabricated structural facts"
        );
        assert!(output.external.findings.is_none());
        assert_eq!(assess(Some(&output)).state, "failed");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn loader_partial_always_makes_the_assessment_partial() {
        // Why this matters: `assess` used to derive state from the reason list
        // alone and ignore the loader's `partial` flag, so a truncated walk
        // could report a complete assessment.
        let dir = temp_skill_dir("partial-flag");
        std::fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
        let a = skill_artifact_with_path(dir.join("SKILL.md").to_string_lossy().as_ref());
        let mut output = run_skill_scanner(&a).unwrap();
        output.partial = true;
        output.incomplete_reasons.clear();

        let assessment = assess(Some(&output));
        assert_eq!(assessment.state, "partial");
        assert!(
            !assessment.reasons.is_empty(),
            "partial must always carry a reason"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn chmod_only_change_invalidates_a_cached_complete_assessment() {
        // Why this matters: cache validation compared only the mode-independent
        // content hashes, so a chmod that turned an inert helper into an
        // unanalyzed executable still reused the cached complete assessment —
        // a live path from currently-incomplete content to a trusted result.
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_skill_dir("chmod-cache");
        std::fs::write(dir.join("SKILL.md"), "# skill\n").unwrap();
        let helper = dir.join("helper.data");
        std::fs::write(&helper, [0xff_u8, 0x00, 0x01]).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o644)).unwrap();

        let artifact = skill_artifact_with_path(dir.join("SKILL.md").to_string_lossy().as_ref());
        let cached = run_skill_scanner(&artifact).unwrap();
        assert_eq!(assess(Some(&cached)).state, "complete");

        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut stale = artifact.clone();
        stale.cached_skill_scan = Some(cached);

        let fresh = output_for(&stale).expect("the skill is still loadable");
        assert_eq!(
            assess(Some(&fresh)).state,
            "partial",
            "a chmod-only change to risky content must not reuse the cache"
        );
        assert!(
            fresh
                .incomplete_reasons
                .contains(&"unanalyzed-content".to_string()),
            "{:?}",
            fresh.incomplete_reasons
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
