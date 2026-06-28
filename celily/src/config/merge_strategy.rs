/// Merge strategy for `Option<T>` fields: profile wins when set.
///
/// Use with `#[merge(strategy = ...)]` on `Option<T>` fields in
/// `#[derive(merge::Merge)]` structs.  When `right` is `Some`, it
/// overwrites `left`; otherwise `left` is unchanged.  This gives
/// profile values precedence over defaults.
pub fn overwrite_some<T>(left: &mut Option<T>, right: Option<T>) {
    if right.is_some() {
        *left = right;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Right wins when Some, left preserved otherwise.
    #[test]
    fn overwrite_some_strategy() {
        let mut left: Option<&str> = Some("left");
        overwrite_some(&mut left, Some("right"));
        assert_eq!(left, Some("right"));

        let mut left: Option<&str> = Some("left");
        overwrite_some(&mut left, None);
        assert_eq!(left, Some("left"));

        let mut left: Option<&str> = None;
        overwrite_some(&mut left, Some("right"));
        assert_eq!(left, Some("right"));

        let mut left: Option<&str> = None;
        overwrite_some(&mut left, None);
        assert_eq!(left, None);
    }
}
