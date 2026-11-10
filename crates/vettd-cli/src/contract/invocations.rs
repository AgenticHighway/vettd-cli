//! Files a skill's own text tells an agent to run (vettd-cli#322).
//!
//! Static and line-oriented. It errs toward naming too many files: a false
//! match only matters when it names a real bundle entry that was not analyzed
//! as text, and then the assessment is marked partial (never affirmative). It
//! stays out of plain prose, though: `Read ./logo.png` is not an execution.

use std::collections::{HashMap, HashSet};

/// Interpreters whose script argument follows. `.` (shell source) is matched
/// on the raw token, before sentence punctuation is trimmed.
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
    "py",
    "uv",
    ".",
];

/// Prose verbs that put the next `./path` in command position.
const RUN_WORDS: &[&str] = &[
    "run", "runs", "execute", "executes", "exec", "invoke", "launch",
];

/// Command separator token produced by `split_args`.
const SEP: &str = ";";

/// Lowercased bundle-relative paths that `text_files` instruct an agent to run.
/// Each is resolved against the skill root and the referencing file's directory.
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

/// Quote-aware split. Backticks and brackets are blanks; `;`, `|` and `&`
/// become a single separator token. Quoted text stays one token.
fn split_args(line: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let flush = |current: &mut String, tokens: &mut Vec<String>| {
        if !current.is_empty() {
            tokens.push(std::mem::take(current));
        }
    };
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() || "`()[]".contains(c) => {
                flush(&mut current, &mut tokens)
            }
            (None, ';' | '|' | '&') => {
                flush(&mut current, &mut tokens);
                if tokens.last().is_some_and(|t| t != SEP) {
                    tokens.push(SEP.to_string());
                }
            }
            (None, c) => current.push(c),
        }
    }
    flush(&mut current, &mut tokens);
    tokens
}

/// Raw path-like tokens a single line asks to run.
fn invoked_tokens(line: &str) -> Vec<String> {
    let tokens = split_args(line);
    let mut found = Vec::new();
    for (i, word) in tokens.iter().enumerate() {
        if word == SEP {
            continue;
        }
        if is_interpreter(word) {
            found.extend(script_arg(&tokens[i + 1..]));
        } else if word.starts_with("./") && word.len() > 2 && in_command_position(&tokens, i) {
            found.push(word.clone());
        }
    }
    found
}

/// First token of the line or of a command, after a list/prompt marker, or
/// right after a "run"-style verb.
fn in_command_position(tokens: &[String], i: usize) -> bool {
    let Some(prev) = i.checked_sub(1).map(|p| tokens[p].as_str()) else {
        return true;
    };
    let digits = prev.trim_end_matches(['.', ')']);
    let numbered =
        digits != prev && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
    prev == SEP
        || numbered
        || matches!(prev, "-" | "*" | ">" | "$" | "#")
        || RUN_WORDS.contains(&prev.trim_end_matches(':').to_ascii_lowercase().as_str())
}

fn is_interpreter(word: &str) -> bool {
    if word == "." {
        return true;
    }
    let word = trim_token(word);
    let base = word.rsplit('/').next().unwrap_or(word).to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base);
    base.starts_with("python") || INTERPRETERS.contains(&base)
}

/// The script among an interpreter's arguments: the first path-like word, or
/// the last word when none looks like a path (`python -X dev run`). Flags and
/// a leading `run` subcommand are skipped, so option values cannot hide it.
fn script_arg(rest: &[String]) -> Option<String> {
    let args: Vec<&str> = rest
        .iter()
        .take_while(|t| *t != SEP)
        .map(|t| trim_token(t))
        .filter(|t| !t.is_empty() && !t.starts_with('-'))
        .collect();
    let args = args.strip_prefix(&["run"][..]).unwrap_or(&args);
    args.iter()
        .find(|t| path_like(t))
        .or_else(|| args.last())
        .map(|t| (*t).to_string())
}

/// Has a directory separator or an alphabetic extension (`3.12` is not one).
fn path_like(token: &str) -> bool {
    token.contains('/')
        || token.rsplit_once('.').is_some_and(|(stem, ext)| {
            !stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphabetic())
        })
}

/// Strip markdown emphasis and sentence punctuation around a token. `$` and
/// `{` are kept: they start a skill-directory placeholder.
fn trim_token(word: &str) -> &str {
    word.trim_matches(['*', '`', '"', '\''])
        .trim_end_matches(['.', ',', ';', ':'])
}

/// A relative in-bundle path, or `None` for URLs, absolute paths, parent
/// escapes and non-paths. A leading `$VAR/` or `{var}/` placeholder is dropped.
fn clean_path(token: &str) -> Option<String> {
    let mut path = trim_token(token).replace('\\', "/");
    if path.starts_with('$') || path.starts_with('{') {
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
    fn finds_each_invocation_form_on_its_own() {
        // One form per line: a neighbouring command must never carry the result.
        for (line, expected) in [
            (
                "Run python payload.dat to complete the task.",
                "payload.dat",
            ),
            ("python3 -u payload.dat", "payload.dat"),
            ("python -W ignore payload.dat", "payload.dat"),
            ("python -X dev payload.dat", "payload.dat"),
            ("uv run --python 3.12 payload.dat", "payload.dat"),
            ("node --no-warnings payload.dat", "payload.dat"),
            ("bash ./payload.dat", "payload.dat"),
            ("python $SKILL_DIR/payload.dat", "payload.dat"),
            ("python ${CLAUDE_SKILL_DIR}/payload.dat.", "payload.dat"),
            ("python {baseDir}/payload.dat", "payload.dat"),
            ("deno run payload.dat", "payload.dat"),
            ("source payload.dat", "payload.dat"),
            (". payload.dat", "payload.dat"),
            ("python \"payload data.dat\"", "payload data.dat"),
            ("python 'my dir/payload.dat'", "my dir/payload.dat"),
            ("bash setup.sh && python -u payload.dat", "payload.dat"),
            ("python -X dev payload", "payload"),
            ("Run ./payload.dat first.", "payload.dat"),
            ("$ ./payload.dat", "payload.dat"),
            ("- ./payload.dat", "payload.dat"),
            ("1. ./payload.dat", "payload.dat"),
            ("make && ./payload.dat", "payload.dat"),
        ] {
            assert!(
                invoked(line).contains(expected),
                "missed {expected:?} in: {line}"
            );
        }
    }

    #[test]
    fn plain_mentions_of_a_file_are_not_invocations() {
        // Paired with the cases above: reading or naming a file must not make a skill partial.
        for line in [
            "Read ./image.png for the diagram.",
            "Summarize ./manual.pdf.",
            "cat ./sample.dat",
            "See ./payload.dat for details.",
            "The payload.dat file holds sample rows.",
            "Open the notes in ./docs/notes.txt",
        ] {
            assert!(invoked(line).is_empty(), "false match in: {line}");
        }
    }

    #[test]
    fn ignores_urls_and_parent_escapes() {
        assert!(!invoked("python https://example.com/x.py").contains("https://example.com/x.py"));
        assert!(invoked("python ../outside.py").is_empty());
    }

    #[test]
    fn resolves_relative_to_the_referencing_file() {
        let mut files = HashMap::new();
        files.insert("docs/howto.md".to_string(), "python run.dat".to_string());
        assert!(invoked_paths(&files).contains("docs/run.dat"));
    }
}
