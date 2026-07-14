mod cli;
mod config;
mod context;
mod util;
mod validate;

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use celily_lib::backend::lxd::LxcBackend;
use celily_lib::{Instance, Limits, NetworkParams, SecretProvider};
use clap::Parser;
use tracing::info;
use tracing::level_filters::LevelFilter;

use crate::cli::Args;
use crate::config::BackendKind;
use crate::context::{resolve_context, resolve_git_identity};
use crate::util::bridge_name;

/// Top-level application logic: load config, resolve context, build
/// the isolated environment via the library, then run the command.
async fn run() -> anyhow::Result<i32> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(LevelFilter::WARN.into())
                .from_env_lossy(),
        )
        .init();

    let args = Args::parse();

    let cwd = env::current_dir().context("failed to get current directory")?;

    let cfg = if let Some(ref profile) = args.profile {
        crate::config::Config::load_with_profile(profile)?
    } else if args.no_profile {
        crate::config::Config::load_default()?
    } else {
        crate::config::Config::load_for_dir(&cwd)?
    };

    let home = PathBuf::from(env::var("HOME").context("$HOME is not set")?);
    let home_canon = home
        .canonicalize()
        .context("failed to canonicalize $HOME")?;
    let config_dir = crate::config::config_dir();

    let ctx = resolve_context(
        &args,
        &cfg,
        &cwd,
        &home,
        &home_canon,
        &config_dir,
        &cfg.allowed_dirs,
        &cfg.allowed_files,
    )?;

    let distro_kind = cfg.distro.unwrap();

    info!(
        name = %ctx.name, image = %ctx.image, kind = ?ctx.kind,
        "building instance"
    );

    // Resolve and verify the secret provider.
    let secret_provider: Option<Box<dyn SecretProvider<Error = celily_lib::SecretError>>> =
        if let Some(provider) = cfg.secret_provider {
            let provider = provider.resolve();
            provider.check_available()?;
            Some(provider)
        } else {
            None
        };

    // Build library types from resolved config + CLI.
    let limits = Limits::builder()
        .cpu(ctx.cpu)
        .memory(ctx.memory.clone())
        .disk(ctx.disk.clone())
        .processes(ctx.processes)
        .maybe_disk_priority(ctx.disk_priority)
        .maybe_memory_enforce(ctx.memory_enforce.clone())
        .maybe_network_ingress(ctx.network_ingress.clone())
        .maybe_network_egress(ctx.network_egress.clone())
        .build();

    let network = NetworkParams::builder()
        .allow(ctx.network_allow.clone())
        .dns(ctx.network_dns)
        .build();

    let bridge = bridge_name(&ctx.image, &ctx.instance_uuid);

    // --- Build and initialize the isolated environment ---
    let mut lxc = match cfg.backend.kind.unwrap_or(BackendKind::Lxd) {
        BackendKind::Lxd => LxcBackend::lxd(),
        BackendKind::Incus => LxcBackend::incus(),
    };

    lxc.project = cfg.backend.project.clone();
    lxc.pool = cfg
        .backend
        .pool
        .clone()
        .unwrap_or_else(|| "default".to_string());

    let lxc = Arc::new(lxc);

    let prepared = Instance::prepare()
        .instance_backend(lxc.clone())
        .network_backend(lxc.clone())
        .image(ctx.image.clone())
        .name(ctx.name.clone())
        .bridge_name(bridge)
        .kind(ctx.kind)
        .distro(distro_kind)
        .limits(limits)
        .network(network)
        .mounts(ctx.mounts.clone())
        .maybe_secret_provider(secret_provider)
        .container_uid(ctx.container_uid)
        .container_gid(ctx.container_gid)
        .exec_uid(ctx.exec_uid)
        .exec_gid(ctx.exec_gid)
        .container_home(ctx.container_home.clone())
        .maybe_raw_idmap(ctx.raw_idmap.clone())
        .security_nesting(ctx.security_nesting)
        .secure_boot(ctx.secure_boot)
        .ephemeral(ctx.ephemeral)
        .keep(!ctx.ephemeral)
        .description(ctx.description.clone())
        .extra_devices(ctx.extra_devices.clone())
        .build();

    let initialized = prepared.init().await?;

    let running = initialized.start().await?;

    // --- Prepare environment ---
    // Proxy env vars are injected by exec() at highest priority.
    let mut env_map = ctx.env_map.clone();

    // Pre-run script.
    if let Some(ref script) = cfg.pre_run {
        let trimmed = script.trim();
        if !trimmed.is_empty() {
            let wrapper = format!(
                "cat > /tmp/celily-pre-run <<'CELILY_EOF'\n{trimmed}\nCELILY_EOF\nchmod +x \
                 /tmp/celily-pre-run && /tmp/celily-pre-run; rc=$?; rm -f /tmp/celily-pre-run; \
                 exit $rc"
            );
            let code = running
                .exec(
                    &["sh".into(), "-c".into(), wrapper],
                    &env_map,
                    Some(&ctx.project_dir),
                )
                .await?;
            if code != 0 {
                return Ok(code);
            }
        }
    }

    // Resolve the command to run -- CLI args as-is, or sh -l if none given.
    let raw_command: Vec<String> = if args.cmd_args.is_empty() {
        vec!["sh".into(), "-l".into()]
    } else {
        args.cmd_args.clone()
    };

    // --- Worktree or direct execution ---
    let code = if ctx.worktree_enabled {
        let worktree_name = args.worktree.as_ref().unwrap();
        let branch_template = cfg.worktree.branch.as_deref().unwrap_or("celily/{name}");
        let branch_name = branch_template.replace("{name}", &worktree_name);

        crate::validate::validate_worktree_preconditions(&cwd, &branch_name).await?;

        let (git_name, git_email) = resolve_git_identity(&args, &cfg.worktree, &cwd).await?;
        env_map.insert("GIT_AUTHOR_NAME".into(), git_name.clone());
        env_map.insert("GIT_AUTHOR_EMAIL".into(), git_email.clone());
        env_map.insert("GIT_COMMITTER_NAME".into(), git_name);
        env_map.insert("GIT_COMMITTER_EMAIL".into(), git_email);

        let auto_commit = cfg.worktree.auto_commit.unwrap_or(true) && !args.no_auto_commit;

        // Pass worktree parameters via environment variables.
        // The script reads these instead of interpolating them into
        // shell source, avoiding injection surface and making the
        // script independently lintable with shellcheck/shfmt.
        env_map.insert("CELILY_WORKTREE_BRANCH".into(), branch_name.clone());
        env_map.insert("CELILY_WORKTREE_NAME".into(), worktree_name.clone());
        env_map.insert(
            "CELILY_WORKTREE_PROJECT".into(),
            ctx.project_dir.to_string_lossy().into_owned(),
        );
        env_map.insert("CELILY_WORKTREE_INSTANCE".into(), ctx.name.clone());
        env_map.insert(
            "CELILY_WORKTREE_AUTO_COMMIT".into(),
            if auto_commit { "1".into() } else { "0".into() },
        );

        let init_script = include_str!("../share/worktree-init.sh");

        let mut full_cmd: Vec<String> = vec![
            "sh".into(),
            "-c".into(),
            init_script.to_string(),
            "--".into(),
        ];
        full_cmd.extend(raw_command);

        running
            .exec(&full_cmd, &env_map, Some(&ctx.project_dir))
            .await?
    } else {
        if ctx.effective_readonly {
            env_map.insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
        }
        running
            .exec(&raw_command, &env_map, Some(&ctx.project_dir))
            .await?
    };

    // Instance dropped here (lxc delete), then isolation bridge torn
    // down. Guaranteed by Instance<Running>'s field declaration order.
    drop(running);
    Ok(code)
}

#[tokio::main]
async fn main() {
    match run().await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            tracing::error!("{:#}", e);
            std::process::exit(1);
        },
    }
}
