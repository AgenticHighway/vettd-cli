//! Files a skill's own text tells an agent to run (vettd-cli#322).
//!
//! Static, line-oriented and deliberately generous: a false match only matters
//! when it names a real bundle entry that was not analyzed as text, and then
//! the assessment is marked partial (never an affirmative result).

use std::collections::{HashMap, HashSet};

/// Interpreters whose next non-flag argument is a script path.
const INTERPRETERS: &[&str] = &[
    "node",
    "nodejs",
    "deno",
    "bun",
    "bash",
    "sh",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "ruby",
    "perl",
    "php",
    "pwsh",
    "powershell",
    "lua",
    "source",
    ".",
    "py",
    "uv",
];

/// Lowercased bundle-relative paths that `text_files` instruct an agent to run.
/// Each token is resolved against the skill root and the referencing file's
/// directory.
pub(super) fn invoked_paths(text_files: &HashMap<String, String>) -> HashSet<String> {
    let mut out = HashSet::new();
    for (file, content) in text_files {
        let dir = file.rsplit_once('/').map_or("", |(dir, _)| dir);
        for line in content.lines() {
            for token in invoked_tokens(line) {
                let Some(path) = clean_path(&token) else {
                    continue;
                };
                out.insert(path.to_lowercase());
                if !dir.is_empty() {
                    out.insert(format!("{dir}/{path}").to_lowercase());
                }
            }
        }
    }
    out
}

/// Raw path-like tokens a single line asks to run.
fn invoked_tokens(line: &str) -> Vec<String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut found = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let name = trim_token(word);
        if name.starts_with("./") && name.len() > 2 {
            found.push(name.to_string());
        }
        if !is_interpreter(name) {
            continue;
        }
        let mut rest = words[i + 1..].iter().map(|w| trim_token(w));
        // `deno run x`, `bun run x`, `uv run x`: skip the subcommand.
        let mut next = rest.next();
        if next == Some("run") {
            next = rest.next();
        }
        while next.is_some_and(|arg| arg.starts_with('-')) {
            next = rest.next();
        }
        if let Some(arg) = next.filter(|arg| !arg.is_empty()) {
            found.push(arg.to_string());
        }
    }
    found
}

fn is_interpreter(word: &str) -> bool {
    let base = word.rsplit('/').next().unwrap_or(word).to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base);
    base.starts_with("python") || INTERPRETERS.contains(&base)
}

/// Strip markdown and shell punctuation around a token.
fn trim_token(word: &str) -> &str {
    word.trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '*' | '$'))
        .trim_end_matches(['.', ',', ';', ':'])
}

/// A relative in-bundle path, or `None` for URLs, absolute paths, parent
/// escapes and non-paths. A leading `$VAR/` or `{var}/` prefix (skill-dir
/// placeholders) is dropped.
fn clean_path(token: &str) -> Option<String> {
    let mut path = trim_token(token).replace('\\', "/");
    if (path.starts_with('$') || path.starts_with('{')) && path.contains('/') {
        path = path.split_once('/')?.1.to_string();
    }
    let path = path.trim_start_matches("./");
    let rejected = path.is_empty()
        || path.contains("://")
        || path.starts_with('/')
        || path.starts_with('~')
        || path.split('/').any(|part| part == "..");
    (!rejected).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoked(text: &str) -> HashSet<String> {
        let mut files = HashMap::new();
        files.insert("SKILL.md".to_string(), text.to_string());
        invoked_paths(&files)
    }

    #[test]
    fn finds_interpreter_and_direct_invocations() {
        for line in [
            "Run python payload.dat to complete the task.",
            "```\npython3 -u payload.dat\n```",
            "node --no-warnings payload.dat",
            "- `bash ./payload.dat`",
            "`$SKILL_DIR/payload.dat`: run it with `deno run payload.dat`",
            "source payload.dat",
            "uv run payload.dat",
            "Run ./payload.dat first.",
            "python ${CLAUDE_SKILL_DIR}/payload.dat.",
        ] {
            assert!(invoked(line).contains("payload.dat"), "missed: {line}");
        }
    }

    #[test]
    fn ignores_urls_parent_escapes_and_plain_mentions() {
        assert!(!invoked("python https://example.com/x.py").contains("https://example.com/x.py"));
        assert!(invoked("python ../outside.py").is_empty());
        assert!(invoked("The payload.dat file holds sample rows.").is_empty());
    }

    #[test]
    fn resolves_relative_to_the_referencing_file() {
        let mut files = HashMap::new();
        files.insert("docs/howto.md".to_string(), "python run.dat".to_string());
        assert!(invoked_paths(&files).contains("docs/run.dat"));
    }
}
