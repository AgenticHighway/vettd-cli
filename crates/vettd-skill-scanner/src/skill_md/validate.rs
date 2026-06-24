use crate::consts::SKILL_NAME_MAX_LENGTH;

/// Returns an error message if the name is invalid, or `None` if valid.
/// Mirrors `validateName` in skill-analyzer.ts.
pub(crate) fn validate_name(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("name field is missing");
    }
    if name.len() > SKILL_NAME_MAX_LENGTH {
        return Some("name exceeds 64-character limit");
    }
    let chars: Vec<char> = name.chars().collect();
    if chars.is_empty() {
        return Some("name field is missing");
    }
    let first = chars[0];
    let last = *chars.last().unwrap();
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        if first == '-' || last == '-' {
            return Some("name must not start or end with a hyphen");
        }
        return Some("name contains invalid characters (only alphanumeric and hyphens allowed)");
    }
    for &c in &chars {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Some(
                "name contains invalid characters (only alphanumeric and hyphens allowed)",
            );
        }
    }
    if name.contains("--") {
        return Some("name must not contain consecutive hyphens");
    }
    None
}
