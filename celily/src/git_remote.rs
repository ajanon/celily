//! Host-side git remote for worktree sessions.
//!
//! The remote's URL runs `git upload-pack` *inside* the instance through
//! git's `ext::` transport. Whatever the sandbox repository's config or hooks
//! make upload-pack do happens in the sandbox: only refs and a pack cross the
//! pipe, and the host's fetch hash-checks every object it receives. The fetch
//! refspec is written here, on the host, so the sandbox cannot choose which
//! ref it updates.

use std::ffi::OsString;
use std::path::Path;

use anyhow::{Context, Result, bail};
use celily_lib::AsyncCommandExt;

use crate::util::git_config;

/// A remote named after the session's branch, so `git fetch <branch>`
/// fetches the sandbox's branch into the host branch of the same name.
pub struct SandboxRemote {
    name: String,
    url: String,
}

impl SandboxRemote {
    /// `exec_argv` turns a command into the argument vector running it in
    /// the instance.
    pub fn new(
        branch: &str,
        worktree: &Path,
        exec_argv: impl FnOnce(&[String]) -> Vec<OsString>,
    ) -> Result<Self> {
        let worktree = worktree
            .to_str()
            .with_context(|| format!("non-UTF-8 worktree path: {}", worktree.display()))?;
        // `upload-pack` instead of ext's `%s` placeholder: the remote can
        // only ever be fetched from, never pushed to.
        let command = ["git", "upload-pack", worktree].map(String::from);
        Ok(Self {
            name: branch.to_owned(),
            url: ext_url(&exec_argv(&command))?,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Write the remote into the host repository's config, replacing a stale
    /// sandbox remote of the same name (e.g. left behind by a crashed run).
    /// Any other remote of that name is the user's, and is left alone.
    pub async fn register(&self, repo: &Path) -> Result<()> {
        let section = format!("remote.{}", self.name);
        if let Some(existing) = git_config(repo, &[&format!("{section}.url")]).await {
            if !existing.starts_with("ext::") {
                bail!(
                    "git remote '{}' already exists and is not a sandbox remote ({existing}); \
                     rename it or use another worktree name",
                    self.name,
                );
            }
            tracing::warn!("replacing existing git remote '{}' ({existing})", self.name);
            git(repo, &["config", "--remove-section", &section]).await?;
        }
        let refspec = format!("+refs/heads/{0}:refs/heads/{0}", self.name);
        let settings = [
            ("url", self.url.as_str()),
            ("fetch", refspec.as_str()),
            // Tags pointing into the fetched history would otherwise be
            // auto-followed into the host's refs/tags/.
            ("tagOpt", "--no-tags"),
            // A global fetch.pruneTags adds refs/tags/*:refs/tags/* to the
            // refspec: sandbox tags would come in, and with fetch.prune,
            // host tags missing from the sandbox would be deleted.
            ("prune", "false"),
            ("pruneTags", "false"),
            // Fetching from the sandbox is always explicit, never part of
            // `git fetch --all` (which may come with --tags).
            ("skipFetchAll", "true"),
        ];
        for (key, value) in settings {
            git(repo, &["config", &format!("{section}.{key}"), value]).await?;
        }
        Ok(())
    }

    /// Remove the remote, unless another run has replaced it meanwhile.
    /// Failures are logged, not returned: a stale remote is harmless.
    pub async fn unregister(&self, repo: &Path) {
        let section = format!("remote.{}", self.name);
        let current = git_config(repo, &[&format!("{section}.url")]).await;
        if current.as_deref() != Some(self.url.as_str()) {
            return;
        }
        if let Err(e) = git(repo, &["config", "--remove-section", &section]).await {
            tracing::warn!("failed to remove git remote '{}': {e:#}", self.name);
        }
    }
}

/// Build an `ext::` URL running `argv`. Git splits the URL on unescaped
/// spaces and expands `%` sequences, so both are escaped.
fn ext_url(argv: &[OsString]) -> Result<String> {
    let parts = argv
        .iter()
        .map(|arg| {
            let arg = arg.to_str().with_context(|| {
                format!("non-UTF-8 argument in exec command: {}", arg.display())
            })?;
            Ok(arg.replace('%', "%%").replace(' ', "% "))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(format!("ext::{}", parts.join(" ")))
}

async fn git(repo: &Path, args: &[&str]) -> Result<()> {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .run()
        .await
        .with_context(|| format!("git {}", args.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_url_escapes_spaces_and_percent() {
        let argv = ["lxc", "exec", "a b", "50%", "--", "git"].map(OsString::from);
        assert_eq!(ext_url(&argv).unwrap(), "ext::lxc exec a% b 50%% -- git");
    }

    /// End to end without an instance: upload-pack runs locally (identity
    /// exec), the rest is the real git behaviour celily relies on.
    #[tokio::test]
    async fn fetch_through_registered_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let (host, sandbox) = (tmp.path().join("host"), tmp.path().join("sand box"));
        let git_ok = |dir: &Path, args: &[&str]| {
            let status = std::process::Command::new("git")
                .args([
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                ])
                .arg("-C")
                .arg(dir)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git_ok(tmp.path(), &["init", "-q", "-b", "main", "host"]);
        git_ok(&host, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git_ok(tmp.path(), &["clone", "-q", "--shared", "host", "sand box"]);
        git_ok(&sandbox, &["switch", "-q", "-c", "celily/foo"]);
        git_ok(&sandbox, &["commit", "-q", "--allow-empty", "-m", "work"]);
        git_ok(&sandbox, &["tag", "sandbox-tag"]);
        // Created after the clone: the sandbox does not have it.
        git_ok(&host, &["tag", "host-tag"]);

        let remote = SandboxRemote::new("celily/foo", &sandbox, |cmd| {
            cmd.iter().map(OsString::from).collect()
        })
        .unwrap();
        // A remote that is not ours must survive untouched.
        git_ok(&host, &["remote", "add", "celily/foo", "/elsewhere"]);
        assert!(
            remote.register(&host).await.is_err(),
            "replaced a user remote"
        );
        assert_eq!(
            git_config(&host, &["remote.celily/foo.url"])
                .await
                .as_deref(),
            Some("/elsewhere")
        );
        git_ok(&host, &["remote", "remove", "celily/foo"]);

        remote.register(&host).await.unwrap();
        // Global pruning must not apply to the sandbox remote.
        git_ok(
            &host,
            &[
                "-c",
                "protocol.ext.allow=user",
                "-c",
                "fetch.prune=true",
                "-c",
                "fetch.pruneTags=true",
                "fetch",
                "-q",
                "celily/foo",
            ],
        );

        let subject = |rev: &str| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&host)
                .args(["log", "-1", "--format=%s", rev])
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        assert_eq!(subject("refs/heads/celily/foo"), "work");
        assert_eq!(
            subject("refs/tags/sandbox-tag"),
            "",
            "tag must not be fetched"
        );
        assert_eq!(
            subject("refs/tags/host-tag"),
            "base",
            "tag must not be pruned"
        );

        remote.unregister(&host).await;
        assert_eq!(git_config(&host, &["remote.celily/foo.url"]).await, None);
        assert_eq!(subject("refs/heads/celily/foo"), "work", "branch survives");
    }
}
