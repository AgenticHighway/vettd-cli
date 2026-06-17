use crate::models::ArtifactReport;

const CAPABILITY_MAP: &[(&str, &str)] = &[
    ("keyword:shell", "shell_execution"),
    ("keyword:browser", "browser_access"),
    ("keyword:api", "external_api_calls"),
    ("keyword:filesystem", "filesystem_access"),
    ("keyword:network", "network_access"),
    ("keyword:execute", "code_execution"),
    ("keyword:docker", "container_runtime"),
    ("keyword:system", "system_prompt"),
    ("keyword:permissions", "permission_scope"),
    ("keyword:dependencies", "dependency_execution"),
    ("keyword:tools", "tool_declarations"),
    ("keyword:secrets", "secret_references"),
];

pub fn derive_capabilities(artifact: &ArtifactReport) -> Vec<String> {
    let mut caps: Vec<String> = Vec::new();
    for signal in &artifact.signals {
        for &(key, capability) in CAPABILITY_MAP {
            if signal == key {
                caps.push(capability.to_string());
            }
        }
    }
    caps.sort();
    caps.dedup();
    caps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_keyword_maps_to_shell_execution() {
        let mut a = ArtifactReport::new("test", 0.8);
        a.signals = vec!["keyword:shell".into()];
        let caps = derive_capabilities(&a);
        assert_eq!(caps, vec!["shell_execution"]);
    }

    #[test]
    fn multiple_keywords_produce_sorted_unique_capabilities() {
        let mut a = ArtifactReport::new("test", 0.8);
        a.signals = vec![
            "keyword:shell".into(),
            "keyword:browser".into(),
            "keyword:api".into(),
            "keyword:shell".into(), // duplicate
        ];
        let caps = derive_capabilities(&a);
        assert_eq!(
            caps,
            vec!["browser_access", "external_api_calls", "shell_execution"]
        );
    }

    #[test]
    fn no_matching_signals_returns_empty() {
        let mut a = ArtifactReport::new("test", 0.8);
        a.signals = vec!["filename_match:foo".into(), "ai_token:openai".into()];
        let caps = derive_capabilities(&a);
        assert!(caps.is_empty());
    }

    #[test]
    fn all_capability_keywords_are_recognized() {
        let mut a = ArtifactReport::new("test", 0.8);
        a.signals = CAPABILITY_MAP.iter().map(|(k, _)| k.to_string()).collect();
        let caps = derive_capabilities(&a);
        // 14 keywords exist but some may share capability names
        assert!(!caps.is_empty());
        assert!(caps.contains(&"shell_execution".to_string()));
        assert!(caps.contains(&"browser_access".to_string()));
        assert!(caps.contains(&"filesystem_access".to_string()));
        assert!(caps.contains(&"network_access".to_string()));
        assert!(caps.contains(&"code_execution".to_string()));
        assert!(caps.contains(&"secret_references".to_string()));
    }
}
