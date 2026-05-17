include!("src/cli_def.rs");

use std::path::Path;
use std::{env, fs};

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir);
    // OUT_DIR: target/{profile}/build/{crate}-{hash}/out
    // Walk up to target/{profile}
    let profile_dir = out_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("failed to find profile directory from OUT_DIR");

    for (shell, ext) in [
        (Shell::Bash, "bash"),
        (Shell::Zsh, "zsh"),
        (Shell::Fish, "fish"),
        (Shell::Elvish, "elv"),
    ] {
        let mut buf = Vec::new();
        generate_completions(shell, &mut buf);
        fs::write(profile_dir.join(format!("celily.{ext}")), &buf).unwrap();
    }
    println!("cargo:rerun-if-changed=src/cli_def.rs");
}
