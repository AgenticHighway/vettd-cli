use std::collections::HashMap;

use crate::models::{ArtifactReport, ScanReport};
use crate::scoring::{
    common_cognitive_signal_weight, secret_signal_weight, shared_signal_weight, ssrf_signal_weight,
};

pub const LITE_MODE_VISIBLE_RESULTS: usize = 3;

fn local_policy_type_base() -> HashMap<&'static str, i32> {
    HashMap::from([
        ("cursor_rules", 4),
        ("agents_md", 4),
        ("skill", 4),
        ("prompt_config", 3),
    ])
}

pub fn local_policy_score(artifact: &ArtifactReport) -> i32 {
    let type_base = local_policy_type_base();
    let has_structured_secret = artifact.signals.iter().any(|s| s.starts_with("secret:"));
    let mut score = *type_base.get(artifact.artifact_type.as_str()).unwrap_or(&1);
    for signal in &artifact.signals {
        let sig = signal.as_str();
        if sig == "credential_exposure_signal" && has_structured_secret {
            continue;
        }
        score += shared_signal_weight(sig).unwrap_or(0);
        score += secret_signal_weight(sig).unwrap_or(0);
        score += ssrf_signal_weight(sig).unwrap_or(0);
        score += common_cognitive_signal_weight(sig).unwrap_or(0);
    }
    score.min(100)
}

pub fn limit_lite_mode_report(
    report: &ScanReport,
    top_n: usize,
) -> (ScanReport, usize, Vec<ArtifactReport>) {
    let mut scored: Vec<(i32, i32, i64, ArtifactReport)> = report
        .artifacts
        .iter()
        .map(|a| {
            (
                local_policy_score(a),
                a.risk_score,
                (a.confidence * 1000.0) as i64,
                a.clone(),
            )
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2)));

    let visible: Vec<ArtifactReport> = scored.iter().take(top_n).map(|t| t.3.clone()).collect();
    let hidden: Vec<ArtifactReport> = scored.iter().skip(top_n).map(|t| t.3.clone()).collect();
    let hidden_count = hidden.len();

    let visible_report = ScanReport {
        scanned_path: report.scanned_path.clone(),
        run_id: report.run_id.clone(),
        timestamp: report.timestamp.clone(),
        artifacts: visible,
    };
    (visible_report, hidden_count, hidden)
}

pub fn locked_summary_counts(artifacts: &[ArtifactReport]) -> serde_json::Value {
    let mut by_type: HashMap<&str, usize> = HashMap::new();
    let mut by_status: HashMap<&str, usize> = HashMap::new();
    let mut by_origin: HashMap<String, usize> = HashMap::new();

    for a in artifacts {
        *by_type.entry(a.artifact_type.as_str()).or_default() += 1;
        *by_status.entry(a.verification_status.as_str()).or_default() += 1;
        let origin = a
            .metadata
            .get("analysis_origin")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        *by_origin.entry(origin).or_default() += 1;
    }

    serde_json::json!({
        "count": artifacts.len(),
        "by_type": by_type,
        "by_status": by_status,
        "by_origin": by_origin,
    })
}

pub fn print_locked_summary(artifacts: &[ArtifactReport]) {
    if artifacts.is_empty() {
        return;
    }
    let summary = locked_summary_counts(artifacts);
    println!("Locked findings summary (lite mode):");
    println!("  Locked findings: {}", summary["count"]);

    if let Some(obj) = summary["by_origin"].as_object() {
        println!("  Analysis handoff:");
        for (k, v) in obj {
            println!("    {}: {}", k, v);
        }
    }
    if let Some(obj) = summary["by_status"].as_object() {
        println!("  Status distribution:");
        for status in &["fail", "conditional_pass", "pass", "pending"] {
            if let Some(v) = obj.get(*status) {
                println!("    {}: {}", status, v);
            }
        }
    }
    if let Some(obj) = summary["by_type"].as_object() {
        println!("  Locked artifact types:");
        for (k, v) in obj {
            println!("    {}: {}", k, v);
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_artifact(
        atype: &str,
        signals: &[&str],
        risk_score: i32,
        confidence: f64,
    ) -> ArtifactReport {
        let mut a = ArtifactReport::new(atype, confidence);
        a.signals = signals.iter().map(|s| s.to_string()).collect();
        a.risk_score = risk_score;
        a
    }

    #[test]
    fn local_policy_score_includes_type_base() {
        let a = make_artifact("cursor_rules", &[], 0, 0.5);
        let score = local_policy_score(&a);
        assert_eq!(score, 4); // cursor_rules base = 4
    }

    #[test]
    fn local_policy_score_adds_signal_weights() {
        let a = make_artifact(
            "cursor_rules",
            &["keyword:shell", "keyword:browser"],
            0,
            0.5,
        );
        let score = local_policy_score(&a);
        assert_eq!(score, 4 + 15 + 10); // base + shell + browser
    }

    #[test]
    fn local_policy_score_uses_structured_secret_weight_without_double_counting() {
        let a = make_artifact(
            "cursor_rules",
            &["credential_exposure_signal", "secret:github:pat"],
            0,
            0.5,
        );
        let score = local_policy_score(&a);
        assert_eq!(score, 29); // 4 + 25, without double counting the generic signal
    }

    #[test]
    fn local_policy_score_adds_ssrf_and_cognitive_weights() {
        let a = make_artifact(
            "prompt_config",
            &["ssrf:metadata:aws", "cognitive_tampering:role_override"],
            0,
            0.5,
        );
        let score = local_policy_score(&a);
        assert_eq!(score, 93); // base 3 + 45 + 45
    }

    #[test]
    fn local_policy_score_caps_at_100() {
        let a = make_artifact(
            "cursor_rules",
            &[
                "credential_exposure_signal",
                "dangerous_combo:shell+network+fs",
                "dangerous_keyword:exfiltrate",
                "dangerous_keyword:steal",
                "keyword:shell",
                "keyword:browser",
            ],
            0,
            0.5,
        );
        let score = local_policy_score(&a);
        assert_eq!(score, 100);
    }

    #[test]
    fn local_policy_score_unknown_type_gets_base_1() {
        let a = make_artifact("unknown_type", &[], 0, 0.5);
        let score = local_policy_score(&a);
        assert_eq!(score, 1);
    }

    #[test]
    fn limit_lite_mode_report_returns_top_n() {
        let mut report = ScanReport::new("/test");
        report.artifacts = vec![
            make_artifact("cursor_rules", &["keyword:shell"], 30, 0.9),
            make_artifact("prompt_config", &[], 5, 0.5),
            make_artifact("agents_md", &["credential_exposure_signal"], 80, 0.95),
        ];
        let (visible, hidden_count, hidden) = limit_lite_mode_report(&report, 2);
        assert_eq!(visible.artifacts.len(), 2);
        assert_eq!(hidden_count, 1);
        assert_eq!(hidden.len(), 1);
    }

    #[test]
    fn limit_lite_mode_highest_risk_first() {
        let mut report = ScanReport::new("/test");
        report.artifacts = vec![
            make_artifact("prompt_config", &[], 5, 0.5),
            make_artifact("agents_md", &["credential_exposure_signal"], 80, 0.95),
        ];
        let (visible, _, _) = limit_lite_mode_report(&report, 1);
        // The credential_exposure artifact should be first (highest policy score)
        assert!(visible.artifacts[0]
            .signals
            .contains(&"credential_exposure_signal".to_string()));
    }

    #[test]
    fn locked_summary_counts_by_type_and_status() {
        let artifacts = vec![
            make_artifact("cursor_rules", &[], 10, 0.8),
            make_artifact("cursor_rules", &[], 15, 0.7),
            make_artifact("mcp_config", &[], 5, 0.9),
        ];
        let summary = locked_summary_counts(&artifacts);
        assert_eq!(summary["count"], 3);
        assert_eq!(summary["by_type"]["cursor_rules"], 2);
        assert_eq!(summary["by_type"]["mcp_config"], 1);
    }
}
