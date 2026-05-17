/// Check whether a name is a valid Linux username.
///
/// Must start with a lowercase letter or underscore, contain only ASCII
/// alphanumeric characters, hyphens, or underscores, and be 1-32
/// characters long.
#[must_use]
pub fn is_valid_username(name: &str) -> bool {
    if name.is_empty() || name.len() > 32 {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_username_accepts_standard() {
        assert!(is_valid_username("dev"));
        assert!(is_valid_username("a"));
        assert!(is_valid_username("user_name"));
        assert!(is_valid_username("user-name"));
        assert!(is_valid_username("_underscore"));
        assert!(is_valid_username("z".repeat(32).as_str()));
    }

    #[test]
    fn is_valid_username_rejects_invalid() {
        assert!(!is_valid_username(""));
        assert!(!is_valid_username("1starts_with_digit"));
        assert!(!is_valid_username("-starts_with_hyphen"));
        assert!(!is_valid_username("UPPERCASE"));
        assert!(!is_valid_username("has spaces"));
        assert!(!is_valid_username("a".repeat(33).as_str()));
        assert!(!is_valid_username("special!char"));
    }
}
