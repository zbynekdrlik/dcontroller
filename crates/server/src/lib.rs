//! dcontroller server: serves the built PWA and a small JSON API.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::http::StatusCode;
use axum::http::{HeaderValue, header};
use axum::routing::{any, get};
use serde::{Deserialize, Serialize};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

/// This build's version, the same string the dashboard shows.
pub const VERSION: &str = env!("DC_VERSION");

/// Server settings, from `~/.config/dcontroller/config.toml` (private, never in the repo).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Address the HTTP server binds.
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,
    /// Directory holding the built PWA (trunk's `dist`).
    pub web_dir: PathBuf,
    /// Network interface on the Dante network (used once Dante features land).
    #[serde(default)]
    pub dante_interface: Option<String>,
    /// Allow a non-loopback `bind`. The server has no login of its own: it
    /// must sit behind Cloudflare Access (cloudflared on the same box reaches
    /// it on loopback), so a public bind is refused unless set explicitly.
    #[serde(default)]
    pub allow_remote: bool,
}

fn default_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8080))
}

impl Config {
    /// Parses a config file's text.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: Self = toml::from_str(text)?;
        if !cfg.bind.ip().is_loopback() && !cfg.allow_remote {
            anyhow::bail!(
                "bind {} is not loopback; the server has no login of its own and must sit behind \
                 Cloudflare Access (set allow_remote = true only if something else guards it)",
                cfg.bind
            );
        }
        Ok(cfg)
    }

    /// Checks what parsing cannot: the web build is present.
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.web_dir.join("index.html").is_file() {
            anyhow::bail!(
                "web_dir {} has no index.html (build the web app first)",
                self.web_dir.display()
            );
        }
        Ok(())
    }

    /// Reads and parses the config file at `path`.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read config {}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| anyhow::anyhow!("parse config {}: {e}", path.display()))
    }
}

/// `GET /healthz` body.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Health {
    pub status: String,
    pub version: String,
}

async fn version() -> &'static str {
    VERSION
}

async fn api_not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn healthz() -> axum::Json<Health> {
    axum::Json(Health {
        status: "ok".into(),
        version: VERSION.into(),
    })
}

/// The whole HTTP app: API routes (unknown `/api/*` is 404), then the PWA
/// from `web_dir` with `index.html` as the fallback for client-side routes.
/// Nothing is cached by the browser, so a deploy shows at once.
pub fn app(web_dir: &Path) -> Router {
    let index = web_dir.join("index.html");
    let spa = ServeDir::new(web_dir).fallback(ServeFile::new(index));
    Router::new()
        .route("/api/version", get(version))
        .route("/api/{*rest}", any(api_not_found))
        .route("/healthz", get(healthz))
        .fallback_service(spa)
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(TraceLayer::new_for_http())
}
