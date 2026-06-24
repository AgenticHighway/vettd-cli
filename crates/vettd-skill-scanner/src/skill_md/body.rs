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
