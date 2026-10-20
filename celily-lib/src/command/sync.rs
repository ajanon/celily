use std::process::Command;
use std::time::Instant;

use tracing::trace;

// -------------------------------------------------------------------
// CommandError
// -------------------------------------------------------------------

/// Error returned by [`CommandExt`] methods when a command fails.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// The command could not be spawned (e.g. binary not found).
    #[error("failed to spawn command: {0}")]
    Io(#[from] std::io::Error),
    /// The command ran but exited with a non-zero code.
    ///
    /// `argv` is a pre-formatted representation of the program and
    /// arguments, including a trailing space when non-empty (so the
    /// Display impl reads naturally).
    #[error("command {argv}exited with code {code:?}{}", .stderr.as_ref().map(|s| format!("\nstderr: {s}")).unwrap_or_default())]
    NonZero {
        argv: String,
        code: Option<i32>,
        stderr: Option<String>,
    },
}

// -------------------------------------------------------------------
// CommandExt
// -------------------------------------------------------------------

/// Extension methods for `std::process::Command`.
pub trait CommandExt {
    /// Run, discarding stdout. On non-zero exit, returns
    /// [`CommandError::NonZero`] carrying stderr.
    fn run(&mut self) -> Result<(), CommandError>;

    /// Run, returning trimmed stdout as `String`. On non-zero exit,
    /// returns [`CommandError::NonZero`] carrying stderr.
    fn run_stdout(&mut self) -> Result<String, CommandError>;

    /// Run, returning the full [`std::process::Output`] on success.
    /// On non-zero exit, returns [`CommandError::NonZero`] carrying
    /// stderr.
    fn run_output(&mut self) -> Result<std::process::Output, CommandError>;
}

impl CommandExt for Command {
    fn run(&mut self) -> Result<(), CommandError> {
        let argv = super::argv_string(self);
        let t = Instant::now();
        let output = self.output()?;
        trace!("{} ({:.2?})", argv.trim(), t.elapsed());
        if !output.status.success() {
            return Err(CommandError::NonZero {
                argv,
                code: output.status.code(),
                stderr: extract_stderr(&output),
            });
        }
        Ok(())
    }

    fn run_stdout(&mut self) -> Result<String, CommandError> {
        let argv = super::argv_string(self);
        let t = Instant::now();
        let output = self.output()?;
        trace!("{} ({:.2?})", argv.trim(), t.elapsed());
        if !output.status.success() {
            return Err(CommandError::NonZero {
                argv,
                code: output.status.code(),
                stderr: extract_stderr(&output),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn run_output(&mut self) -> Result<std::process::Output, CommandError> {
        let argv = super::argv_string(self);
        let t = Instant::now();
        let output = self.output()?;
        trace!("{} ({:.2?})", argv.trim(), t.elapsed());
        if !output.status.success() {
            return Err(CommandError::NonZero {
                argv,
                code: output.status.code(),
                stderr: extract_stderr(&output),
            });
        }
        Ok(output)
    }
}

// -------------------------------------------------------------------
// helpers
// -------------------------------------------------------------------

/// Extract non-empty stderr from an [`Output`] as `Option<String>`.
pub(crate) fn extract_stderr(output: &std::process::Output) -> Option<String> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        None
    } else {
        Some(stderr.to_string())
    }
}

// -------------------------------------------------------------------
// tests
// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    // ---- CommandExt::run ----

    #[test]
    fn run_success() {
        Command::new("true").run().expect("true should succeed");
    }

    #[test]
    fn run_failure_empty_stderr() {
        let err = Command::new("false").run().unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(1));
        assert!(
            stderr.is_none(),
            "empty stderr should be None, got {stderr:?}"
        );
    }

    #[test]
    fn run_failure_captures_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'bang' >&2; exit 2"]);
        let err = cmd.run().unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(2));
        assert_eq!(stderr.as_deref(), Some("bang"));
    }

    #[test]
    fn run_io_error() {
        let err = Command::new("/no/such/binary").run().unwrap_err();
        assert!(
            matches!(err, CommandError::Io(_)),
            "expected CommandError::Io, got {err:?}"
        );
    }

    // ---- CommandExt::run_stdout ----

    #[test]
    fn run_stdout_success() {
        let out = Command::new("echo")
            .arg("hello")
            .run_stdout()
            .expect("echo should succeed");
        assert_eq!(out, "hello");
    }

    #[test]
    fn run_stdout_trims_trailing_newline() {
        let out = Command::new("printf")
            .arg("foo")
            .run_stdout()
            .expect("printf should succeed");
        assert_eq!(out, "foo");
    }

    #[test]
    fn run_stdout_failure_captures_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'bad' >&2; exit 3"]);
        let err = cmd.run_stdout().unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(3));
        assert_eq!(stderr.as_deref(), Some("bad"));
    }

    #[test]
    fn run_failure_includes_argv_in_display() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exit 5"]);
        let err = cmd.run().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("`sh -c exit 5`"),
            "expected argv in error message: {msg}"
        );
    }

    #[test]
    fn run_failure_argv_no_args() {
        let err = Command::new("false").run().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("`false`"),
            "expected program in error message: {msg}"
        );
    }

    // ---- CommandExt::run_output ----

    #[test]
    fn run_output_success() {
        let mut cmd = Command::new("printf");
        cmd.arg("hello");
        let output = cmd.run_output().expect("printf should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "hello");
    }

    #[test]
    fn run_output_failure_captures_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'fail' >&2; exit 4"]);
        let err = cmd.run_output().unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(4));
        assert_eq!(stderr.as_deref(), Some("fail"));
    }
}
