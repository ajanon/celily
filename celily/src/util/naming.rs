use std::path::Path;

use uuid::Uuid;

/// Build an LXD-safe instance name from the working directory, image, and a
/// pre-generated UUID.
///
/// Format: `<image-prefix>-<path-fragment>-<uuid8>`. Max 63 characters.
pub fn instance_name(cwd: &Path, home: &Path, image: &str, uuid: &Uuid) -> String {
    let uuid8 = &uuid.to_string()[..8];
    let image_prefix = sanitized_image_prefix(image);

    let path_str = if let Ok(rel) = cwd.strip_prefix(home) {
        format!("home-{}", rel.display())
    } else {
        cwd.to_string_lossy().trim_start_matches('/').to_string()
    };

    let raw: String = path_str
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    let mut path_part = String::with_capacity(raw.len());
    let mut prev_hyphen = false;
    for c in raw.chars() {
        if c == '-' {
            if !prev_hyphen && !path_part.is_empty() {
                path_part.push('-');
            }
            prev_hyphen = true;
        } else {
            path_part.push(c);
            prev_hyphen = false;
        }
    }
    let path_part = path_part.trim_matches('-').to_string();

    let prefix_overhead = image_prefix.len() + 2;
    let suffix_overhead = 9;
    let path_max = 63usize.saturating_sub(prefix_overhead + suffix_overhead);

    let path_part = if path_part.len() > path_max {
        let tail = &path_part[path_part.len() - path_max..];
        tail.trim_start_matches('-').to_string()
    } else {
        path_part
    };

    format!("{image_prefix}-{path_part}-{uuid8}")
}

/// Build a short bridge name (max 15 characters) from the image name and
/// instance UUID.
///
/// Format: `<prefix6>-<uuid8>`. Linux bridge names are limited to 15 chars.
pub fn bridge_name(image: &str, uuid: &Uuid) -> String {
    let uuid8 = &uuid.to_string()[..8];
    let prefix = sanitized_image_prefix(image);
    let prefix: String = prefix.chars().take(6).collect();
    format!("{prefix}-{uuid8}")
}

/// Derive a short, LXD-safe prefix from an image name.
///
/// Takes the last component after the final `:` or `/`, keeps only
/// alphanumeric characters, and truncates to 20 characters.
fn sanitized_image_prefix(image: &str) -> String {
    let last = image.rsplit(&['/', ':']).next().unwrap_or(image);
    let prefix: String = last.chars().filter(char::is_ascii_alphanumeric).collect();
    if prefix.is_empty() {
        "inst".to_string()
    } else if prefix.len() > 20 {
        prefix[..20].to_string()
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const U: Uuid = Uuid::from_bytes([
        0xde, 0xad, 0xbe, 0xef, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56,
        0x78,
    ]);

    #[test]
    fn instance_name_truncates_uuid() {
        let name = instance_name(Path::new("/tmp/x"), Path::new("/home/user"), "img", &U);
        assert!(name.ends_with("-deadbeef"), "got: {name}");
    }

    #[test]
    fn bridge_name_truncates_uuid() {
        let name = bridge_name("myimage", &U);
        assert_eq!(name, "myimag-deadbeef");
    }

    #[test]
    fn instance_name_format() {
        let cwd = Path::new("/home/user/projects/celily");
        let home = Path::new("/home/user");
        let name = instance_name(cwd, home, "celily", &U);
        assert!(name.starts_with("celily-"), "starts with celily-: {name}");
        assert!(name.len() >= 13, "min length: {name}");
        let parts: Vec<&str> = name.split('-').collect();
        assert!(parts.len() >= 3, "at least 3 parts: {name}");
        let uuid_part = parts.last().unwrap();
        assert_eq!(uuid_part.len(), 8, "uuid part len: {name}");
        assert!(
            uuid_part.chars().all(|c| c.is_ascii_hexdigit()),
            "uuid hex: {name}"
        );
    }

    #[test]
    fn instance_name_under_63_chars() {
        let cwd = Path::new("/home/user/short");
        let home = Path::new("/home/user");
        let name = instance_name(cwd, home, "images:ubuntu/24.04", &U);
        assert!(name.len() <= 63, "name is {} chars: {name}", name.len());
        assert!(name.starts_with("2404-"), "starts with 2404-: {name}");
    }

    #[test]
    fn instance_name_long_path_truncated() {
        let long_segment = "a".repeat(80);
        let cwd = PathBuf::from(format!("/home/user/{long_segment}"));
        let home = Path::new("/home/user");
        let name = instance_name(&cwd, home, "pi", &U);
        assert!(name.len() <= 63, "name is {} chars: {name}", name.len());
    }

    #[test]
    fn instance_name_image_prefix() {
        let cwd = Path::new("/home/user/projects/celily");
        let home = Path::new("/home/user");
        let name = instance_name(cwd, home, "images:archlinux/current/default", &U);
        assert!(name.starts_with("default-"), "got name: {name}");
    }

    #[test]
    fn instance_name_outside_home() {
        let cwd = Path::new("/tmp/build");
        let home = Path::new("/home/user");
        let name = instance_name(cwd, home, "celily", &U);
        assert!(name.starts_with("celily-"));
        assert!(name.len() <= 63);
    }

    #[test]
    fn bridge_name_length() {
        let name = bridge_name("images:archlinux/current/default", &U);
        assert_eq!(
            name.len(),
            15,
            "bridge name is {} chars: {name}",
            name.len()
        );
        assert_eq!(name, "defaul-deadbeef");
    }

    #[test]
    fn bridge_name_from_short_image() {
        let name = bridge_name("celily", &U);
        assert!(name.len() <= 15);
        assert_eq!(name, "celily-deadbeef");
    }

    #[test]
    fn sanitized_image_prefix_basic() {
        assert_eq!(sanitized_image_prefix("celily"), "celily");
        assert_eq!(sanitized_image_prefix("images:ubuntu/22.04"), "2204");
        assert_eq!(
            sanitized_image_prefix("images:archlinux/current/default"),
            "default"
        );
    }

    #[test]
    fn sanitized_image_prefix_empty_fallback() {
        assert_eq!(sanitized_image_prefix("://"), "inst");
    }

    #[test]
    fn sanitized_image_prefix_long() {
        let very_long = format!("images:{}", "a".repeat(100));
        let prefix = sanitized_image_prefix(&very_long);
        assert_eq!(prefix.len(), 20);
    }
}
