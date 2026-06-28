include!("cli_def.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_mount() {
        let m = parse_cli_mount("/src:/dst").unwrap();
        assert_eq!(m.source, PathBuf::from("/src"));
        assert_eq!(m.target, PathBuf::from("/dst"));
        assert_eq!(m.access, AccessMode::ReadOnly);
    }

    #[test]
    fn readwrite_mount() {
        let m = parse_cli_mount("/src:/dst:readwrite").unwrap();
        assert_eq!(m.access, AccessMode::ReadWrite);
    }

    #[test]
    fn readonly_mount() {
        let m = parse_cli_mount("/src:/dst:readonly").unwrap();
        assert_eq!(m.access, AccessMode::ReadOnly);
    }

    #[test]
    fn relative_paths() {
        let m = parse_cli_mount("./foo:./bar").unwrap();
        assert_eq!(m.source, PathBuf::from("./foo"));
        assert_eq!(m.target, PathBuf::from("./bar"));
    }

    #[test]
    fn too_few_parts() {
        let err = parse_cli_mount("/only-source").unwrap_err();
        assert!(err.contains("expected 'source:target"));
    }

    #[test]
    fn empty_string() {
        let err = parse_cli_mount("").unwrap_err();
        assert!(err.contains("expected 'source:target"));
    }

    #[test]
    fn unknown_flag() {
        let err = parse_cli_mount("/src:/dst:ro").unwrap_err();
        assert!(err.contains("invalid mount flag"));
    }

    #[test]
    fn extra_colons_ignored() {
        let err = parse_cli_mount("a:b:c:d").unwrap_err();
        assert!(err.contains("invalid mount flag"));
    }
}
