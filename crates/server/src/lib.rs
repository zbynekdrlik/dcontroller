//! dcontroller server: serves the built PWA and a small JSON API.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::routing::get;
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
}

fn default_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8080))
}

impl Config {
    /// Parses a config file's text.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(text)?)
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

async fn healthz() -> axum::Json<Health> {
    axum::Json(Health {
        status: "ok".into(),
        version: VERSION.into(),
    })
}

/// The whole HTTP app: API routes, then the PWA from `web_dir` with
/// `index.html` as the fallback for client-side routes.
pub fn app(web_dir: &Path) -> Router {
    let index = web_dir.join("index.html");
    let spa = ServeDir::new(web_dir).fallback(ServeFile::new(index));
    Router::new()
        .route("/api/version", get(version))
        .route("/healthz", get(healthz))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .fallback_service(spa)
        .layer(TraceLayer::new_for_http())
}
