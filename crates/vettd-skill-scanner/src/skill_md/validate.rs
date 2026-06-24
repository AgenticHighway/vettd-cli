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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_name_is_invalid() {
        assert!(validate_name("").is_some());
    }

    #[test]
    fn name_over_64_chars_is_invalid() {
        let long = "a".repeat(65);
        assert!(validate_name(&long).is_some());
    }

    #[test]
    fn leading_hyphen_is_invalid() {
        assert!(validate_name("-my-skill").is_some());
    }

    #[test]
    fn trailing_hyphen_is_invalid() {
        assert!(validate_name("my-skill-").is_some());
    }

    #[test]
    fn consecutive_hyphens_invalid() {
        assert!(validate_name("my--skill").is_some());
    }

    #[test]
    fn invalid_char_rejected() {
        assert!(validate_name("my_skill").is_some());
        assert!(validate_name("my skill").is_some());
        assert!(validate_name("my.skill").is_some());
    }

    #[test]
    fn valid_names_accepted() {
        assert!(validate_name("my-skill").is_none());
        assert!(validate_name("skill123").is_none());
        assert!(validate_name("a").is_none());
        assert!(validate_name("abc-def-123").is_none());
    }
}
