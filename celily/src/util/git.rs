use std::path::Path;

use celily_lib::AsyncCommandExt;

/// Run `git -C <dir> config <args>` on the host and return the value.
///
/// Returns `None` if the key is unset, the command fails, or the value is
/// empty. Pass e.g. `["--type=bool", key]` to let git normalize the value.
pub async fn git_config(dir: &Path, args: &[&str]) -> Option<String> {
    let val = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("config")
        .args(args)
        .run_stdout()
        .await
        .ok()?;
    if val.is_empty() { None } else { Some(val) }
}
