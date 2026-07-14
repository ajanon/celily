mod r#async;
mod sync;

use std::process::Command;

pub use r#async::AsyncCommandExt;
pub use sync::*;

/// Build a backtick-quoted, space-joined representation of a Command's
/// program and arguments, with a trailing space.
///
/// e.g. `` `lxc start my-instance` `` (note trailing space).
fn argv_string(cmd: &Command) -> String {
    let prog = cmd.get_program().to_string_lossy();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if args.is_empty() {
        format!("`{prog}` ")
    } else {
        format!("`{prog} {}` ", args.join(" "))
    }
}
