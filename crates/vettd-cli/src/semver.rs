//! Minimal `major.minor.patch` version comparison, shared across modules.

use std::cmp::Ordering;

/// Parse a `major.minor.patch` version string into a tuple.
///
/// A leading `v` is stripped. Returns `None` for any other format.
pub fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

/// Compare two version strings. Returns `None` when either is unparseable.
///
/// The result follows `Ord` conventions: `Less` means `a < b`.
pub fn cmp(a: &str, b: &str) -> Option<Ordering> {
    Some(parse(a)?.cmp(&parse(b)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_versions() {
        assert_eq!(cmp("2.4.0", "2.4.0"), Some(Ordering::Equal));
    }

    #[test]
    fn a_less_than_b() {
        // compiled = 2.3.0, server = 2.4.0 → compiled is Less → "behind"
        assert_eq!(cmp("2.3.0", "2.4.0"), Some(Ordering::Less));
    }

    #[test]
    fn a_greater_than_b() {
        // compiled = 2.5.0, server = 2.4.0 → compiled is Greater → "ahead"
        assert_eq!(cmp("2.5.0", "2.4.0"), Some(Ordering::Greater));
    }

    #[test]
    fn v_prefix_stripped() {
        assert_eq!(cmp("v2.4.0", "v2.4.0"), Some(Ordering::Equal));
        assert_eq!(cmp("v2.3.0", "2.4.0"), Some(Ordering::Less));
    }

    #[test]
    fn minor_version_ordering() {
        assert_eq!(cmp("2.9.0", "2.10.0"), Some(Ordering::Less));
    }

    #[test]
    fn unparseable_returns_none() {
        assert_eq!(cmp("2.4", "2.4.0"), None);
        assert_eq!(cmp("", "2.4.0"), None);
        assert_eq!(cmp("2.4.0", "not-a-version"), None);
    }
}
