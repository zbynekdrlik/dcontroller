use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use dcontroller::{Config, VERSION, app};
use tracing::info;

fn config_path(arg: Option<std::ffi::OsString>) -> anyhow::Result<PathBuf> {
    if let Some(p) = arg {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").context("HOME not set and no config path given")?;
    Ok(PathBuf::from(home).join(".config/dcontroller/config.toml"))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    // `--check-config <path>`: parse the config, print its bind address, exit.
    // The installer uses it to trust only a config the server accepts.
    if first.as_deref() == Some(std::ffi::OsStr::new("--check-config")) {
        let path = config_path(args.next())?;
        let cfg = Config::load(&path)?;
        println!("{}", cfg.bind);
        return Ok(());
    }
    serve(config_path(first)?)
}

#[tokio::main]
async fn serve(path: PathBuf) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();
    let cfg = Config::load(&path)?;
    info!(version = VERSION, config = %path.display(), bind = %cfg.bind,
        web_dir = %cfg.web_dir.display(), dante_interface = ?cfg.dante_interface, "starting");
    cfg.validate()?;
    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("bind {}", cfg.bind))?;
    info!(addr = %listener.local_addr()?, "listening");
    let stop = Arc::new(AtomicBool::new(false));
    let dante = dcontroller::dante::start(cfg.dante_interface.clone(), Arc::clone(&stop));
    info!(changes_log = %cfg.changes_log.display(), "changes log");
    let shared = dcontroller::Shared {
        dante,
        changes: Arc::new(dcontroller::change::Changes::new(cfg.changes_log.clone())),
        latency: Arc::default(),
    };
    axum::serve(listener, app(&cfg.web_dir, shared))
        .with_graceful_shutdown(async {
            shutdown_signal().await;
            info!("shutdown");
        })
        .await?;
    stop.store(true, Ordering::Relaxed);
    Ok(())
}

/// SIGINT or SIGTERM (systemd stops the service with SIGTERM).
async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install the SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
