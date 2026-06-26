//! `vettd rules` subcommand — manage custom TOML detection rule files.
//!
//! Commands:
//!   list      — show installed rules
//!   add       — copy a rule file into ~/.vettd/rules/
//!   remove    — delete a rule by name
//!   validate  — parse and report errors in a rule file without installing it

use crate::rule_engine::{default_rules_dir, load_rule_file_pub, DetectionRule};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Entry points (CLI-facing, call process::exit on error)
// ---------------------------------------------------------------------------

pub fn cmd_list(json: bool) {
    let dir = match rules_dir_or_exit() {
        Some(d) => d,
        None => return,
    };

    if !dir.exists() {
        if json {
            println!("[]");
        } else {
            eprintln!(
                "No rules directory found ({}). No custom rules installed.",
                dir.display()
            );
        }
        return;
    }

    let entries = toml_entries(&dir);

    if entries.is_empty() {
        if json {
            println!("[]");
        } else {
            eprintln!("No rules installed in {}.", dir.display());
            eprintln!("Use `vettd rules add <file.toml>` to install one.");
        }
        return;
    }

    if json {
        #[derive(Serialize)]
        struct RuleEntry {
            file: String,
            name: String,
            artifact_type: String,
            confidence: f64,
        }
        let out: Vec<RuleEntry> = entries
            .iter()
            .filter_map(|path| {
                load_rule_file_pub(path).ok().map(|rule| RuleEntry {
                    file: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    name: rule.detector.name,
                    artifact_type: rule.detector.artifact_type,
                    confidence: rule.match_config.confidence,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        eprintln!("{} custom rule(s) in {}:", entries.len(), dir.display());
        for path in &entries {
            match load_rule_file_pub(path) {
                Ok(rule) => print_rule_summary(path, &rule),
                Err(e) => eprintln!(
                    "  {} [INVALID: {}]",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    e
                ),
            }
        }
    }
}

pub fn cmd_add(source: &Path) {
    if !source.exists() {
        eprintln!("Error: file not found: {}", source.display());
        std::process::exit(1);
    }
    if source.extension().and_then(|e| e.to_str()) != Some("toml") {
        eprintln!("Error: rule files must have a .toml extension.");
        std::process::exit(1);
    }

    let dir = ensure_rules_dir();
    match install_rule(source, &dir) {
        Ok(dest) => eprintln!("Installed: {}", dest.display()),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub fn cmd_remove(name: &str) {
    let dir = match rules_dir_or_exit() {
        Some(d) => d,
        None => return,
    };

    if !dir.exists() {
        eprintln!("No rules directory found. Nothing to remove.");
        return;
    }

    match remove_rule(&dir, name) {
        Ok(path) => eprintln!("Removed: {}", path.display()),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub fn cmd_validate(source: &Path, json: bool) {
    if !source.exists() {
        if json {
            let msg = format!("file not found: {}", source.display());
            println!("{}", serde_json::json!({"valid": false, "error": msg}));
        } else {
            eprintln!("Error: file not found: {}", source.display());
        }
        std::process::exit(1);
    }

    match load_rule_file_pub(source) {
        Ok(rule) => {
            if json {
                #[derive(Serialize)]
                struct ValidateOutput {
                    valid: bool,
                    name: String,
                    artifact_type: String,
                    filenames: Vec<String>,
                    suffixes: Vec<String>,
                    confidence: f64,
                    has_keywords: bool,
                    has_deep_keywords: bool,
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ValidateOutput {
                        valid: true,
                        name: rule.detector.name,
                        artifact_type: rule.detector.artifact_type,
                        filenames: rule.match_config.filenames,
                        suffixes: rule.match_config.suffixes,
                        confidence: rule.match_config.confidence,
                        has_keywords: rule.keywords.is_some(),
                        has_deep_keywords: rule.deep_keywords.is_some(),
                    })
                    .unwrap_or_default()
                );
            } else {
                eprintln!("OK: '{}' is valid.", rule.detector.name);
                eprintln!("  artifact_type : {}", rule.detector.artifact_type);
                eprintln!("  filenames     : {:?}", rule.match_config.filenames);
                eprintln!("  suffixes      : {:?}", rule.match_config.suffixes);
                eprintln!("  confidence    : {}", rule.match_config.confidence);
                if rule.keywords.is_some() {
                    eprintln!("  keywords      : yes");
                }
                if rule.deep_keywords.is_some() {
                    eprintln!("  deep_keywords : yes");
                }
            }
        }
        Err(e) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"valid": false, "error": e.to_string()})
                );
            } else {
                eprintln!("INVALID: {e}");
            }
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Core logic (testable — no process::exit)
// ---------------------------------------------------------------------------

/// Validate and copy `source` into `dest_dir`. Returns the installed path.
pub(crate) fn install_rule(source: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    ensure_regular_rule_file(source, "source")?;

    // Validate before touching the dest directory
    let rule = load_rule_file_pub(source).map_err(|e| format!("rule validation failed — {e}"))?;
    eprintln!("Rule '{}' validated OK.", rule.detector.name);

    let dest = dest_dir.join(source.file_name().ok_or("source has no filename")?);

    if dest.exists() {
        ensure_regular_rule_file(&dest, "destination")?;
        eprintln!(
            "Warning: {} already exists. Overwriting.",
            dest.file_name().unwrap_or_default().to_string_lossy()
        );
    }

    fs::copy(source, &dest).map_err(|e| format!("could not install rule: {e}"))?;
    Ok(dest)
}

/// Delete a rule file by name (stem or full filename). Returns the removed path.
pub(crate) fn remove_rule(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = find_rule_file(dir, name)
        .ok_or_else(|| format!("no rule named '{}' found in {}", name, dir.display()))?;
    fs::remove_file(&path).map_err(|e| format!("could not remove {}: {e}", path.display()))?;
    Ok(path)
}

/// Return sorted list of `.toml` files in `dir`.
pub(crate) fn toml_entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
        .filter(|p| {
            fs::symlink_metadata(p)
                .map(|m| m.file_type().is_file())
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    paths
}

/// Find a rule file by stem (`terraform-ai`) or full name (`terraform-ai.toml`).
pub(crate) fn find_rule_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let stem = name.trim_end_matches(".toml");
    for entry in toml_entries(dir) {
        let file_stem = entry
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let file_name = entry
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if file_stem == stem || file_name == name {
            return Some(entry);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn rules_dir_or_exit() -> Option<PathBuf> {
    match default_rules_dir() {
        Some(d) => Some(d),
        None => {
            eprintln!("Error: could not determine home directory.");
            std::process::exit(1);
        }
    }
}

fn ensure_rules_dir() -> PathBuf {
    let dir = match default_rules_dir() {
        Some(d) => d,
        None => {
            eprintln!("Error: could not determine home directory.");
            std::process::exit(1);
        }
    };
    if !dir.exists() {
        if let Err(e) = fs::create_dir_all(&dir) {
            eprintln!("Error: could not create {}: {e}", dir.display());
            std::process::exit(1);
        }
    }
    dir
}

fn print_rule_summary(path: &Path, rule: &DetectionRule) {
    let file = path.file_name().unwrap_or_default().to_string_lossy();
    eprintln!(
        "  {} — {} (artifact_type: {})",
        file, rule.detector.description, rule.detector.artifact_type
    );
}

fn ensure_regular_rule_file(path: &Path, label: &str) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("could not inspect {label} file {}: {e}", path.display()))?;

    if meta.file_type().is_symlink() {
        return Err(format!(
            "{label} file {} must not be a symlink",
            path.display()
        ));
    }
    if !meta.file_type().is_file() {
        return Err(format!(
            "{label} file {} must be a regular file",
            path.display()
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    fn make_symlink(src: &Path, dest: &Path) {
        std::os::unix::fs::symlink(src, dest).unwrap();
    }

    /// Create a uniquely-named temp directory that is removed when the guard drops.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "ah_rules_test_{}_{}",
                label,
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // Minimal valid rule TOML
    const VALID_TOML: &str = r#"
[detector]
name = "test_rule"
description = "A test rule"
artifact_type = "test_artifact"

[match]
suffixes = [".test"]
confidence = 0.5
"#;

    // Rule missing required detector.name
    const MISSING_NAME_TOML: &str = r#"
[detector]
name = ""
description = "bad"
artifact_type = "x"

[match]
suffixes = [".x"]
confidence = 0.5
"#;

    // Rule with no match patterns
    const NO_MATCH_TOML: &str = r#"
[detector]
name = "no_match"
description = "no match patterns"
artifact_type = "x"

[match]
confidence = 0.5
"#;

    // Rule with syntax error
    const INVALID_TOML: &str = "this is not \x00 valid toml :::";

    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).expect("write file");
        path
    }

    // -----------------------------------------------------------------------
    // toml_entries
    // -----------------------------------------------------------------------

    #[test]
    fn toml_entries_empty_dir() {
        let dir = TempDir::new("entries_empty");
        assert!(toml_entries(dir.path()).is_empty());
    }

    #[test]
    fn toml_entries_filters_non_toml() {
        let dir = TempDir::new("entries_filter");
        write_file(dir.path(), "rule.toml", VALID_TOML);
        write_file(dir.path(), "readme.md", "# docs");
        write_file(dir.path(), "script.sh", "#!/bin/sh");

        let entries = toml_entries(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].file_name().unwrap().to_str().unwrap(),
            "rule.toml"
        );
    }

    #[test]
    fn toml_entries_skips_non_regular_toml_paths() {
        let dir = TempDir::new("entries_non_regular");
        write_file(dir.path(), "rule.toml", VALID_TOML);
        fs::create_dir(dir.path().join("folder.toml")).unwrap();

        let entries = toml_entries(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].file_name().unwrap().to_str().unwrap(),
            "rule.toml"
        );
    }

    #[test]
    fn toml_entries_returns_sorted() {
        let dir = TempDir::new("entries_sorted");
        write_file(dir.path(), "z-rule.toml", VALID_TOML);
        write_file(dir.path(), "a-rule.toml", VALID_TOML);
        write_file(dir.path(), "m-rule.toml", VALID_TOML);

        let entries = toml_entries(dir.path());
        let names: Vec<_> = entries
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, ["a-rule.toml", "m-rule.toml", "z-rule.toml"]);
    }

    // -----------------------------------------------------------------------
    // find_rule_file
    // -----------------------------------------------------------------------

    #[test]
    fn find_rule_file_by_stem() {
        let dir = TempDir::new("find_stem");
        write_file(dir.path(), "my-rule.toml", VALID_TOML);

        let found = find_rule_file(dir.path(), "my-rule");
        assert!(found.is_some());
        assert_eq!(
            found.unwrap().file_name().unwrap().to_str().unwrap(),
            "my-rule.toml"
        );
    }

    #[test]
    fn find_rule_file_by_full_name() {
        let dir = TempDir::new("find_fullname");
        write_file(dir.path(), "my-rule.toml", VALID_TOML);

        let found = find_rule_file(dir.path(), "my-rule.toml");
        assert!(found.is_some());
    }

    #[test]
    fn find_rule_file_not_found() {
        let dir = TempDir::new("find_missing");
        write_file(dir.path(), "other.toml", VALID_TOML);

        assert!(find_rule_file(dir.path(), "my-rule").is_none());
        assert!(find_rule_file(dir.path(), "my-rule.toml").is_none());
    }

    #[test]
    fn find_rule_file_empty_dir() {
        let dir = TempDir::new("find_empty");
        assert!(find_rule_file(dir.path(), "anything").is_none());
    }

    // -----------------------------------------------------------------------
    // install_rule
    // -----------------------------------------------------------------------

    #[test]
    fn install_rule_copies_file() {
        let src_dir = TempDir::new("install_src");
        let dest_dir = TempDir::new("install_dest");
        let source = write_file(src_dir.path(), "valid.toml", VALID_TOML);

        let result = install_rule(&source, dest_dir.path());
        assert!(result.is_ok(), "install failed: {:?}", result);

        let dest = result.unwrap();
        assert!(dest.exists());
        assert_eq!(dest.file_name().unwrap().to_str().unwrap(), "valid.toml");
        assert_eq!(fs::read_to_string(&dest).unwrap(), VALID_TOML);
    }

    #[test]
    fn install_rule_overwrites_existing() {
        let src_dir = TempDir::new("install_overwrite_src");
        let dest_dir = TempDir::new("install_overwrite_dest");
        let source = write_file(src_dir.path(), "valid.toml", VALID_TOML);

        // Pre-populate dest
        write_file(dest_dir.path(), "valid.toml", "old content");

        let result = install_rule(&source, dest_dir.path());
        assert!(result.is_ok());

        let dest = result.unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), VALID_TOML);
    }

    #[test]
    fn install_rule_rejects_invalid_toml() {
        let src_dir = TempDir::new("install_invalid");
        let dest_dir = TempDir::new("install_invalid_dest");
        let source = write_file(src_dir.path(), "bad.toml", INVALID_TOML);

        let result = install_rule(&source, dest_dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("validation failed"));
    }

    #[test]
    fn install_rule_rejects_missing_name() {
        let src_dir = TempDir::new("install_no_name");
        let dest_dir = TempDir::new("install_no_name_dest");
        let source = write_file(src_dir.path(), "noname.toml", MISSING_NAME_TOML);

        let result = install_rule(&source, dest_dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn install_rule_rejects_no_match_patterns() {
        let src_dir = TempDir::new("install_no_match");
        let dest_dir = TempDir::new("install_no_match_dest");
        let source = write_file(src_dir.path(), "nomatch.toml", NO_MATCH_TOML);

        let result = install_rule(&source, dest_dir.path());
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn install_rule_rejects_symlink_source() {
        let src_dir = TempDir::new("install_symlink_src");
        let dest_dir = TempDir::new("install_symlink_dest");
        let real = write_file(src_dir.path(), "real.toml", VALID_TOML);
        let symlink = src_dir.path().join("linked.toml");
        make_symlink(&real, &symlink);

        let result = install_rule(&symlink, dest_dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be a symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn install_rule_rejects_symlink_destination() {
        let src_dir = TempDir::new("install_dest_symlink_src");
        let dest_dir = TempDir::new("install_dest_symlink_dest");
        let source = write_file(src_dir.path(), "valid.toml", VALID_TOML);
        let real_dest = write_file(dest_dir.path(), "real-target.toml", VALID_TOML);
        let symlink_dest = dest_dir.path().join("valid.toml");
        make_symlink(&real_dest, &symlink_dest);

        let result = install_rule(&source, dest_dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("destination file"));
    }

    // -----------------------------------------------------------------------
    // remove_rule
    // -----------------------------------------------------------------------

    #[test]
    fn remove_rule_by_stem() {
        let dir = TempDir::new("remove_stem");
        write_file(dir.path(), "my-rule.toml", VALID_TOML);

        let result = remove_rule(dir.path(), "my-rule");
        assert!(result.is_ok());
        assert!(!dir.path().join("my-rule.toml").exists());
    }

    #[test]
    fn remove_rule_by_full_name() {
        let dir = TempDir::new("remove_fullname");
        write_file(dir.path(), "my-rule.toml", VALID_TOML);

        let result = remove_rule(dir.path(), "my-rule.toml");
        assert!(result.is_ok());
        assert!(!dir.path().join("my-rule.toml").exists());
    }

    #[test]
    fn remove_rule_not_found_returns_error() {
        let dir = TempDir::new("remove_missing");
        write_file(dir.path(), "other.toml", VALID_TOML);

        let result = remove_rule(dir.path(), "nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no rule named"));
    }

    #[test]
    fn remove_rule_only_removes_named_file() {
        let dir = TempDir::new("remove_one");
        write_file(dir.path(), "keep.toml", VALID_TOML);
        write_file(dir.path(), "remove-me.toml", VALID_TOML);

        remove_rule(dir.path(), "remove-me").unwrap();

        assert!(!dir.path().join("remove-me.toml").exists());
        assert!(dir.path().join("keep.toml").exists());
    }

    // -----------------------------------------------------------------------
    // install + remove round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn install_then_remove_round_trip() {
        let src_dir = TempDir::new("rt_src");
        let rules_dir = TempDir::new("rt_rules");
        let source = write_file(src_dir.path(), "round-trip.toml", VALID_TOML);

        install_rule(&source, rules_dir.path()).unwrap();
        assert_eq!(toml_entries(rules_dir.path()).len(), 1);

        remove_rule(rules_dir.path(), "round-trip").unwrap();
        assert!(toml_entries(rules_dir.path()).is_empty());
    }

    // -----------------------------------------------------------------------
    // load_rule_file_pub (validate path)
    // -----------------------------------------------------------------------

    #[test]
    fn validate_accepts_valid_rule() {
        let dir = TempDir::new("validate_ok");
        let path = write_file(dir.path(), "ok.toml", VALID_TOML);
        assert!(load_rule_file_pub(&path).is_ok());
    }

    #[test]
    fn validate_rejects_syntax_error() {
        let dir = TempDir::new("validate_syntax");
        let path = write_file(dir.path(), "bad.toml", INVALID_TOML);
        let result = load_rule_file_pub(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("parse error"));
    }

    #[test]
    fn validate_rejects_empty_name() {
        let dir = TempDir::new("validate_name");
        let path = write_file(dir.path(), "noname.toml", MISSING_NAME_TOML);
        let result = load_rule_file_pub(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("detector.name is required"));
    }

    #[test]
    fn validate_rejects_no_match_patterns() {
        let dir = TempDir::new("validate_nopat");
        let path = write_file(dir.path(), "nopat.toml", NO_MATCH_TOML);
        let result = load_rule_file_pub(&path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("match.filenames or match.suffixes"));
    }

    #[test]
    fn validate_real_example_rule() {
        // Ensure the committed example file stays valid
        let path = std::path::Path::new("examples/rules/terraform-ai.toml");
        if path.exists() {
            let result = load_rule_file_pub(path);
            assert!(result.is_ok(), "terraform-ai.toml is invalid: {:?}", result);
            let rule = result.unwrap();
            assert_eq!(rule.detector.artifact_type, "terraform_config");
            assert!(rule.keywords.is_some());
        }
    }

    #[test]
    fn validate_real_internal_tool_rule() {
        let path = std::path::Path::new("examples/rules/internal-tool.toml");
        if path.exists() {
            let result = load_rule_file_pub(path);
            assert!(
                result.is_ok(),
                "internal-tool.toml is invalid: {:?}",
                result
            );
            let rule = result.unwrap();
            assert_eq!(rule.detector.artifact_type, "internal_tool_config");
        }
    }
}
