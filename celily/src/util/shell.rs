/// Escape a string for safe use inside single-quoted shell strings.
///
/// Replaces internal `'` characters with `'\''` (end single-quote, escaped
/// literal quote, resume single-quote) and wraps the result in single quotes.
#[must_use]
pub fn shell_escape(val: &str) -> String {
    let escaped = val.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_plain_string() {
        assert_eq!(shell_escape("hello"), "'hello'");
    }

    #[test]
    fn shell_escape_empty() {
        assert_eq!(shell_escape(""), "''");
    }

    #[test]
    fn shell_escape_with_single_quote() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_escape_multiple_quotes() {
        assert_eq!(shell_escape("'a''b'"), "''\\''a'\\'''\\''b'\\'''");
    }

    #[test]
    fn shell_escape_special_chars() {
        let val = "pass$word!";
        assert_eq!(shell_escape(val), format!("'{val}'"));
    }
}
