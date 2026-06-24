pub(crate) mod body;
pub(crate) mod validate;

/// frontmatter fields and body extracted from a `SKILL.md` file.
pub(crate) struct ParsedSkillMd {
    /// value of the `name:` field; `"unknown"` if absent or unparseable.
    pub(crate) name: String,
    /// value of the `description:` field; empty string if absent.
    pub(crate) description: String,
    /// value of the `repository:` field; empty string if absent.
    pub(crate) repository: String,
    /// everything after the closing `---` fence, with leading blank lines stripped.
    pub(crate) body: String,
}

/// Parse a SKILL.md string into its frontmatter fields and body.
///
/// Mirrors vettd's `parseFrontmatter` in skill-analyzer.ts.
/// Handles simple scalar `key: value` frontmatter; nested objects and
/// list values are skipped (indented lines are ignored).
pub(crate) fn parse_skill_md(content: &str) -> ParsedSkillMd {
    let empty = ParsedSkillMd {
        name: "unknown".to_string(),
        description: String::new(),
        repository: String::new(),
        body: content.to_string(),
    };

    if !content.starts_with("---\n") {
        return empty;
    }
    let rest = &content[4..]; // skip opening "---\n"

    let close_seq = "\n---";
    let Some(close_pos) = rest.find(close_seq) else {
        return empty;
    };

    let after_dashes = &rest[close_pos + close_seq.len()..];
    let trimmed_after = after_dashes.trim_start_matches([' ', '\t']);
    if !trimmed_after.is_empty()
        && !trimmed_after.starts_with('\n')
        && !trimmed_after.starts_with('\r')
    {
        return empty;
    }

    let raw = &rest[..close_pos];
    let body = if let Some(stripped) = trimmed_after.strip_prefix('\n') {
        stripped.trim_start_matches('\n').to_string()
    } else {
        String::new()
    };

    let mut name = "unknown".to_string();
    let mut description = String::new();
    let mut repository = String::new();

    let fm_lines: Vec<&str> = raw.lines().collect();
    let mut idx = 0;
    while idx < fm_lines.len() {
        let line = fm_lines[idx];
        if line.starts_with(' ') || line.starts_with('\t') {
            idx += 1;
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }
        let Some(colon_pos) = trimmed.find(':') else {
            idx += 1;
            continue;
        };
        let key = trimmed[..colon_pos].trim();
        let inline_value = trimmed[colon_pos + 1..].trim();
        idx += 1;

        let value: String = if !inline_value.is_empty() {
            strip_quotes(inline_value).to_string()
        } else {
            let indent = line.len() - line.trim_start().len();
            let mut block: Vec<&str> = Vec::new();
            while idx < fm_lines.len() {
                let child = fm_lines[idx];
                if child.trim().is_empty() {
                    block.push(child);
                    idx += 1;
                    continue;
                }
                let child_indent = child.len() - child.trim_start().len();
                if child_indent <= indent {
                    break;
                }
                block.push(child);
                idx += 1;
            }
            block
                .iter()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.trim())
                .collect::<Vec<_>>()
                .join(" ")
        };

        match key {
            "name" => name = value,
            "description" => description = value,
            "repository" => repository = value,
            _ => {}
        }
    }

    ParsedSkillMd {
        name,
        description,
        repository,
        body,
    }
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}
