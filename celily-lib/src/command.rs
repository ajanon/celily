use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
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
        let argv = argv_string(self);
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
        let argv = argv_string(self);
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
        let argv = argv_string(self);
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
// ShutdownStatus
// -------------------------------------------------------------------

/// Outcome of a [`ChildExt::shutdown`] call.
#[derive(Debug)]
pub enum ShutdownStatus {
    /// Process had already exited before we attempted shutdown
    /// (crash, OOM, external kill, etc.).
    AlreadyExited(std::process::ExitStatus),
    /// Shutdown completed; the process has been reaped.
    /// Inspect the [`ExitStatus`](std::process::ExitStatus) to
    /// determine whether the exit was clean.
    Exited(std::process::ExitStatus),
    /// Failed to reap the process after shutdown.
    WaitFailure(std::io::Error),
}

// -------------------------------------------------------------------
// ChildExt
// -------------------------------------------------------------------

/// Extension methods for `std::process::Child`.
pub trait ChildExt {
    /// Politely shut down a child process.
    ///
    /// Sends SIGTERM, waits up to `grace` for the process to exit
    /// gracefully, then escalates to SIGKILL if it hasn't.
    ///
    /// Returns [`ShutdownStatus::AlreadyExited`] if the process was
    /// already dead before we touched it -- the caller can decide
    /// whether that's an error.
    fn shutdown(&mut self, grace: Duration) -> ShutdownStatus;
}

impl ChildExt for std::process::Child {
    fn shutdown(&mut self, grace: Duration) -> ShutdownStatus {
        // Check if already dead before we touch it.
        // (Tiny race: the process could die between this check and the
        // SIGTERM below, but the window is microseconds.)
        match self.try_wait() {
            Ok(Some(status)) => return ShutdownStatus::AlreadyExited(status),
            Ok(None) => {},
            Err(e) => return ShutdownStatus::WaitFailure(e),
        }

        // Send SIGTERM for a graceful shutdown.
        let pid = Pid::from_raw(self.id() as i32);
        if kill(pid, Signal::SIGTERM).is_err() {
            // SIGTERM failed; fall back to SIGKILL immediately.
            let _ = self.kill();
            return match self.wait() {
                Ok(s) => ShutdownStatus::Exited(s),
                Err(e) => ShutdownStatus::WaitFailure(e),
            };
        }

        // Poll for graceful exit within the grace period.
        let deadline = Instant::now() + grace;
        loop {
            match self.try_wait() {
                Ok(Some(s)) => return ShutdownStatus::Exited(s),
                Ok(None) if Instant::now() < deadline => {
                    sleep(Duration::from_millis(50));
                },
                Ok(None) => {
                    // Grace period expired; escalate to SIGKILL.
                    let _ = self.kill();
                    return match self.wait() {
                        Ok(s) => ShutdownStatus::Exited(s),
                        Err(e) => ShutdownStatus::WaitFailure(e),
                    };
                },
                Err(e) => {
                    // try_wait failed; escalate to SIGKILL and try wait().
                    let _ = self.kill();
                    return match self.wait() {
                        Ok(s) => ShutdownStatus::Exited(s),
                        Err(_) => ShutdownStatus::WaitFailure(e),
                    };
                },
            }
        }
    }
}

// -------------------------------------------------------------------
// run_parallel
// -------------------------------------------------------------------

/// Run multiple commands concurrently using a scoped thread per command.
///
/// Returns one result per command in the original order. A command that
/// fails does not cancel the others -- every command completes.
pub fn run_parallel(commands: Vec<Command>) -> Vec<Result<(), CommandError>> {
    if commands.is_empty() {
        return Vec::new();
    }
    std::thread::scope(|s| {
        let handles: Vec<_> = commands
            .into_iter()
            .map(|mut cmd| s.spawn(move || cmd.run()))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

// -------------------------------------------------------------------
// helpers
// -------------------------------------------------------------------

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

/// Extract non-empty stderr from an [`Output`] as `Option<String>`.
fn extract_stderr(output: &std::process::Output) -> Option<String> {
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

    // ---- run_parallel ----

    #[test]
    fn run_parallel_all_succeed() {
        let cmds = vec![
            Command::new("true"),
            Command::new("true"),
            Command::new("true"),
        ];
        let results = run_parallel(cmds);
        assert_eq!(results.len(), 3);
        for r in results {
            r.expect("true should succeed");
        }
    }

    #[test]
    fn run_parallel_one_fails() {
        let cmds = vec![
            Command::new("true"),
            Command::new("false"),
            Command::new("true"),
        ];
        let results = run_parallel(cmds);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[2].is_ok());
    }

    #[test]
    fn run_parallel_empty() {
        let results = run_parallel(vec![]);
        assert!(results.is_empty());
    }
}
