use std::process::Stdio;
use std::time::Instant;

use tokio::io::AsyncWriteExt;
use tokio::process::Command as TokioCommand;
use tracing::trace;

use super::sync::{CommandError, extract_stderr};

/// Extension methods for `tokio::process::Command`.
///
/// Mirrors [`super::CommandExt`] exactly, but async.
#[allow(async_fn_in_trait)]
pub trait AsyncCommandExt {
    async fn run(&mut self) -> Result<(), CommandError>;
    async fn run_stdout(&mut self) -> Result<String, CommandError>;
    async fn run_output(&mut self) -> Result<std::process::Output, CommandError>;
    /// Run, piping `input` to the command's stdin. Stdout is discarded:
    /// celily's stdout belongs to the command. A non-zero exit is reported
    /// over the broken pipe it may have caused.
    async fn run_with_stdin(&mut self, input: &[u8]) -> Result<(), CommandError>;
}

impl AsyncCommandExt for TokioCommand {
    async fn run(&mut self) -> Result<(), CommandError> {
        let argv = super::argv_string(self.as_std());
        let t = Instant::now();
        let output = self.output().await?;
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

    async fn run_stdout(&mut self) -> Result<String, CommandError> {
        let argv = super::argv_string(self.as_std());
        let t = Instant::now();
        let output = self.output().await?;
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

    async fn run_output(&mut self) -> Result<std::process::Output, CommandError> {
        let argv = super::argv_string(self.as_std());
        let t = Instant::now();
        let output = self.output().await?;
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

    async fn run_with_stdin(&mut self, input: &[u8]) -> Result<(), CommandError> {
        let argv = super::argv_string(self.as_std());
        let t = Instant::now();
        self.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = self.spawn().map_err(CommandError::Io)?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // Write while waiting: a child filling its stderr pipe before
        // reading stdin would otherwise deadlock. Dropping stdin sends EOF.
        let write = async move { stdin.write_all(input).await };
        let (write_result, output) = tokio::join!(write, child.wait_with_output());
        let output = output.map_err(CommandError::Io)?;
        trace!("{} ({:.2?})", argv.trim(), t.elapsed());
        if !output.status.success() {
            return Err(CommandError::NonZero {
                argv,
                code: output.status.code(),
                stderr: extract_stderr(&output),
            });
        }
        write_result.map_err(CommandError::Io)?;
        Ok(())
    }
}

// -------------------------------------------------------------------
// tests
// -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use tokio::process::Command;

    use super::*;

    // ---- AsyncCommandExt::run ----

    #[tokio::test]
    async fn run_success() {
        Command::new("true")
            .run()
            .await
            .expect("true should succeed");
    }

    #[tokio::test]
    async fn run_failure_empty_stderr() {
        let err = Command::new("false").run().await.unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(1));
        assert!(
            stderr.is_none(),
            "empty stderr should be None, got {stderr:?}"
        );
    }

    #[tokio::test]
    async fn run_failure_captures_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'bang' >&2; exit 2"]);
        let err = cmd.run().await.unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(2));
        assert_eq!(stderr.as_deref(), Some("bang"));
    }

    #[tokio::test]
    async fn run_io_error() {
        let err = Command::new("/no/such/binary").run().await.unwrap_err();
        assert!(
            matches!(err, CommandError::Io(_)),
            "expected CommandError::Io, got {err:?}"
        );
    }

    // ---- AsyncCommandExt::run_stdout ----

    #[tokio::test]
    async fn run_stdout_success() {
        let out = Command::new("echo")
            .arg("hello")
            .run_stdout()
            .await
            .expect("echo should succeed");
        assert_eq!(out, "hello");
    }

    #[tokio::test]
    async fn run_stdout_trims_trailing_newline() {
        let out = Command::new("printf")
            .arg("foo")
            .run_stdout()
            .await
            .expect("printf should succeed");
        assert_eq!(out, "foo");
    }

    #[tokio::test]
    async fn run_stdout_failure_captures_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'bad' >&2; exit 3"]);
        let err = cmd.run_stdout().await.unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(3));
        assert_eq!(stderr.as_deref(), Some("bad"));
    }

    #[tokio::test]
    async fn run_failure_includes_argv_in_display() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exit 5"]);
        let err = cmd.run().await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("`sh -c exit 5`"),
            "expected argv in error message: {msg}"
        );
    }

    #[tokio::test]
    async fn run_failure_argv_no_args() {
        let err = Command::new("false").run().await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("`false`"),
            "expected program in error message: {msg}"
        );
    }

    // ---- AsyncCommandExt::run_output ----

    #[tokio::test]
    async fn run_output_success() {
        let mut cmd = Command::new("printf");
        cmd.arg("hello");
        let output = cmd.run_output().await.expect("printf should succeed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout, "hello");
    }

    #[tokio::test]
    async fn run_output_failure_captures_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo 'fail' >&2; exit 4"]);
        let err = cmd.run_output().await.unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(4));
        assert_eq!(stderr.as_deref(), Some("fail"));
    }

    // ---- AsyncCommandExt::run_with_stdin ----

    #[tokio::test]
    async fn run_with_stdin_feeds_the_child() {
        let mut cmd = Command::new("grep");
        cmd.arg("-q").arg("hello");
        cmd.run_with_stdin(b"nope\nhello there\n")
            .await
            .expect("grep -q hello should find its stdin");
    }

    #[tokio::test]
    async fn run_with_stdin_does_not_deadlock_on_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "head -c 262144 /dev/zero >&2; cat >/dev/null"]);
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            cmd.run_with_stdin(&vec![b'x'; 256 * 1024]),
        )
        .await
        .expect("no deadlock")
        .expect("child succeeds");
    }

    #[tokio::test]
    async fn run_with_stdin_failure_captures_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "read line; echo \"got: $line\" >&2; exit 3"]);
        let err = cmd.run_with_stdin(b"payload\n").await.unwrap_err();
        let CommandError::NonZero { code, stderr, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(3));
        assert_eq!(stderr.as_deref(), Some("got: payload"));
    }

    #[tokio::test]
    async fn run_with_stdin_child_failure_wins_over_broken_pipe() {
        // More than a pipe buffer, never read: the write breaks.
        let big = vec![b'x'; 256 * 1024];
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exit 4"]);
        let err = cmd.run_with_stdin(&big).await.unwrap_err();
        let CommandError::NonZero { code, .. } = err else {
            panic!("expected CommandError::NonZero, got {err:?}");
        };
        assert_eq!(code, Some(4));
    }
}
