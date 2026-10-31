use std::path::PathBuf;

use anyhow::Context;
use dcontroller::{Config, VERSION, app};
use tracing::info;

fn config_path() -> anyhow::Result<PathBuf> {
    if let Some(p) = std::env::args_os().nth(1) {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").context("HOME not set and no config path given")?;
    Ok(PathBuf::from(home).join(".config/dcontroller/config.toml"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();
    let path = config_path()?;
    let cfg = Config::load(&path)?;
    info!(version = VERSION, config = %path.display(), bind = %cfg.bind,
        web_dir = %cfg.web_dir.display(), dante_interface = ?cfg.dante_interface, "starting");
    cfg.validate()?;
    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .with_context(|| format!("bind {}", cfg.bind))?;
    info!(addr = %listener.local_addr()?, "listening");
    axum::serve(listener, app(&cfg.web_dir))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutdown");
        })
        .await?;
    Ok(())
}
