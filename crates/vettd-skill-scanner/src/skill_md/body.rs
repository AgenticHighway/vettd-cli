use std::sync::OnceLock;

use regex::Regex;

pub(crate) fn has_examples(body: &str) -> bool {
    let lower = body.to_lowercase();
    body.contains("```")
        || lower.contains("# example")
        || lower.contains("## example")
        || lower.contains("# sample")
        || lower.contains("## sample")
        || lower.contains("# demo")
        || lower.contains("## demo")
        || lower.contains("**input**")
        || lower.contains("**output**")
        || lower.contains("**before**")
        || lower.contains("**after**")
        || lower.contains("**example**")
        || lower.contains("**good**")
        || lower.contains("**bad**")
}

// Mirrors vettd's hasGotchas: /##?\s*gotcha/i || /##?\s*common\s+mistakes/i
pub(crate) fn has_gotchas(body: &str) -> bool {
    static GOTCHAS_RE: OnceLock<Regex> = OnceLock::new();
    let re = GOTCHAS_RE.get_or_init(|| {
        Regex::new(r"(?i)##?\s*(?:gotcha|common\s+mistakes)").expect("bad gotchas re")
    });
    re.is_match(body)
}

// Mirrors vettd's hasChecklist: /- \[ \]/ || /##?\s*checklist/i
pub(crate) fn has_checklist(body: &str) -> bool {
    if body.contains("- [ ]") {
        return true;
    }
    static CHECKLIST_RE: OnceLock<Regex> = OnceLock::new();
    let re = CHECKLIST_RE
        .get_or_init(|| Regex::new(r"(?im)^##?\s*checklist").expect("bad checklist re"));
    re.is_match(body)
}

// Mirrors vettd's hasValidation: /validat/i.test(body) || /##?\s*verification/i.
// Note: matches "invalidation" and similar — this is a vettd bug reproduced as-is.
/// returns true if the body appears to describe a validation or verification step.
///
/// # Note
/// matches any substring containing "validat" (e.g. "invalidation") — intentional parity
/// with a known bug in vettd's `hasValidation` check. do not fix without updating the web app.
pub(crate) fn has_validation(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("validat")
        || lower.lines().any(|l| {
            l.trim_start_matches('#')
                .trim()
                .to_lowercase()
                .starts_with("verification")
                && l.trim_start().starts_with('#')
        })
}

pub(crate) fn has_workflow(body: &str) -> bool {
    let lower = body.to_lowercase();
    for heading in &[
        "# workflow",
        "## workflow",
        "# steps",
        "## steps",
        "# instructions",
        "## instructions",
        "# procedure",
        "## procedure",
        "# process",
        "## process",
        "# how to",
        "## how to",
        "# usage",
        "## usage",
        "# guidelines",
        "## guidelines",
    ] {
        if lower.contains(heading) {
            return true;
        }
    }
    if let Some(pos) = lower.find("step") {
        let after = lower[pos + 4..].trim_start_matches(' ');
        if after.starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
    }
    for line in body.lines() {
        if line.starts_with(|c: char| c.is_ascii_digit()) {
            let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());
            if rest.starts_with(". ") {
                return true;
            }
        }
        let t = line.trim_start();
        if (t.starts_with("- ") || t.starts_with("* ")) && t.to_lowercase().contains("**step") {
            return true;
        }
    }
    false
}

pub(crate) fn has_usage_context(description: &str) -> bool {
    let lower = description.to_lowercase();
    if lower.contains("use this ") || lower.contains("use when") || lower.contains("use for") {
        return true;
    }
    if let Some(pos) = lower.find("when ") {
        let rest = &lower[pos..];
        if rest.contains("need")
            || rest.contains("want")
            || rest.contains("ask")
            || rest.contains("mention")
        {
            return true;
        }
    }
    false
}

pub(crate) fn has_external_url(content: &str) -> bool {
    content.contains("http://") || content.contains("https://")
}

pub(crate) fn has_cli_hint(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("argparse")
        || content.contains("--help")
        || lower.contains("argumentparser")
        || lower.contains(".option(")
        || lower.contains("yargs")
        || lower.contains("commander")
        || lower.contains("process.argv")
        || lower.contains("deno.args")
        || lower.contains("sys.argv")
        || lower.contains("click.command")
        || lower.contains("click.group")
        || lower.contains("typer.")
        || content.contains("if __name__ == '__main__'")
        || content.contains("if __name__ == \"__main__\"")
}

/// heuristically determines whether a script is a CLI entry point rather than a library or helper.
///
/// uses path depth, file extension, basename/subdirectory blocklists, and CLI indicator
/// detection to distinguish runnable scripts from support files. shell scripts directly under
/// `scripts/` always return `true`; files in helper subdirectories require a positive
/// `has_cli_hint` result.
///
/// # Parameters
/// - `path` — normalized relative path within the skill package.
/// - `content` — decoded file content.
pub(crate) fn is_likely_cli_script(path: &str, content: &str) -> bool {
    if !path.starts_with("scripts/") {
        return false;
    }
    let lower = path.to_lowercase();
    let basename = lower.rsplit('/').next().unwrap_or("");
    const NON_CLI_BASENAMES: &[&str] = &[
        "__init__.py",
        "utils.py",
        "helper.py",
        "helpers.py",
        "base.py",
        "constants.py",
    ];
    if NON_CLI_BASENAMES.contains(&basename) {
        return false;
    }
    let ext = lower.rsplit('.').next().unwrap_or("");

    const NON_CLI_EXTS: &[&str] = &[
        "json", "xml", "xsd", "yaml", "yml", "toml", "txt", "md", "csv", "tsv",
    ];
    if NON_CLI_EXTS.contains(&ext) {
        return false;
    }

    if lower.contains("/schemas/")
        || lower.contains("/templates/")
        || lower.contains("/fixtures/")
        || lower.contains("/examples/")
        || lower.contains("/testdata/")
    {
        return false;
    }

    if lower.contains("/helpers/") || lower.contains("/lib/") || lower.contains("/validators/") {
        return has_cli_hint(content);
    }

    if matches!(ext, "sh" | "bash" | "zsh") {
        return true;
    }

    let depth = path.split('/').count();
    depth <= 2 || has_cli_hint(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- has_examples ---
    #[test]
    fn has_examples_detects_code_fence() {
        assert!(has_examples("some text\n```bash\necho hi\n```"));
    }
    #[test]
    fn has_examples_detects_example_heading() {
        assert!(has_examples("## Example\nsome content"));
    }
    #[test]
    fn has_examples_false_when_absent() {
        assert!(!has_examples("Just a plain description."));
    }

    // --- has_gotchas ---
    #[test]
    fn has_gotchas_detects_heading() {
        assert!(has_gotchas("## Gotchas\nWatch out for X."));
        assert!(has_gotchas("# Common Mistakes\n- Do not do Y."));
    }
    #[test]
    fn has_gotchas_false_when_absent() {
        assert!(!has_gotchas("No gotchas here."));
    }

    // --- has_checklist ---
    #[test]
    fn has_checklist_detects_task_item() {
        assert!(has_checklist("- [ ] step one\n- [ ] step two"));
    }
    #[test]
    fn has_checklist_detects_heading() {
        assert!(has_checklist("## Checklist\nDo the thing."));
    }
    #[test]
    fn has_checklist_false_when_absent() {
        assert!(!has_checklist("No checklist here."));
    }

    // --- has_validation ---
    #[test]
    fn has_validation_detects_validat_substring() {
        assert!(has_validation("Run the validator to check."));
    }
    #[test]
    fn has_validation_parity_bug_matches_invalidation() {
        // Intentional parity with vettd hasValidation bug — do not fix.
        assert!(has_validation("This step handles cache invalidation."));
    }
    #[test]
    fn has_validation_detects_verification_heading() {
        assert!(has_validation("## Verification\nCheck the output."));
    }
    #[test]
    fn has_validation_false_when_absent() {
        assert!(!has_validation("No checks here."));
    }

    // --- has_workflow ---
    #[test]
    fn has_workflow_detects_numbered_list() {
        assert!(has_workflow("1. First step\n2. Second step"));
    }
    #[test]
    fn has_workflow_detects_heading() {
        assert!(has_workflow("## Steps\nDo this first."));
    }
    #[test]
    fn has_workflow_false_when_absent() {
        assert!(!has_workflow("Just a short note."));
    }

    // --- has_usage_context ---
    #[test]
    fn has_usage_context_detects_use_this() {
        assert!(has_usage_context(
            "Use this skill when you need to format JSON."
        ));
    }
    #[test]
    fn has_usage_context_detects_when_need() {
        assert!(has_usage_context("when you need a quick summary"));
    }
    #[test]
    fn has_usage_context_false_when_absent() {
        assert!(!has_usage_context("Formats and sorts data."));
    }

    // --- has_external_url ---
    #[test]
    fn has_external_url_detects_https() {
        assert!(has_external_url("See https://example.com for more."));
    }
    #[test]
    fn has_external_url_false_when_absent() {
        assert!(!has_external_url("No links here."));
    }

    // --- is_likely_cli_script ---
    #[test]
    fn shell_script_directly_under_scripts_is_cli() {
        assert!(is_likely_cli_script("scripts/run.sh", "#!/bin/bash"));
    }
    #[test]
    fn non_scripts_path_is_not_cli() {
        assert!(!is_likely_cli_script("src/main.py", "import sys"));
    }
    #[test]
    fn data_file_in_scripts_is_not_cli() {
        assert!(!is_likely_cli_script("scripts/config.json", "{}"));
    }
    #[test]
    fn helper_subdir_without_cli_hint_is_not_cli() {
        assert!(!is_likely_cli_script(
            "scripts/helpers/util.py",
            "def helper(): pass"
        ));
    }
    #[test]
    fn helper_subdir_with_cli_hint_is_cli() {
        assert!(is_likely_cli_script(
            "scripts/helpers/main.py",
            "import argparse\nparser = argparse.ArgumentParser()"
        ));
    }
}
