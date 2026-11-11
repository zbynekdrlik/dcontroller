//! dcontroller server: serves the built PWA and a small JSON API.

pub mod change;
pub mod dante;
pub mod flows;
pub mod history;
pub mod latency;
pub mod meters;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::extract::State;
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
    /// Network interface on the Dante network: discovery and channel reads
    /// run from its IPv4 address. Unset = no Dante discovery.
    #[serde(default)]
    pub dante_interface: Option<String>,
    /// Allow a non-loopback `bind`. The server has no login of its own: it
    /// must sit behind Cloudflare Access (cloudflared on the same box reaches
    /// it on loopback), so a public bind is refused unless set explicitly.
    #[serde(default)]
    pub allow_remote: bool,
    /// The changes log: every subscription change, one JSON line each.
    #[serde(default = "default_changes_log")]
    pub changes_log: PathBuf,
    /// The latency history (SQLite), recorded for every receiver.
    #[serde(default = "default_history_db")]
    pub history_db: PathBuf,
    /// Further Dante devices on this PC, by the unix user each runs as
    /// (presenter's dlink device: `presenter`): the Dante NIC guard lets them
    /// send on the Dante interface and opens the device ports
    /// (dstack docs/two-networks.md section 3). Read by the installer.
    #[serde(default)]
    pub guard_users: Vec<String>,
}

/// A unix user name the guard accepts: a letter or `_`, then letters,
/// digits, `_` or `-`, at most 32.
pub fn valid_user(name: &str) -> bool {
    let mut c = name.chars();
    name.len() <= 32
        && c.next().is_some_and(|f| f.is_ascii_lowercase() || f == '_')
        && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_' || x == '-')
}

fn default_history_db() -> PathBuf {
    PathBuf::from("/var/lib/dcontroller/history.db")
}

fn default_changes_log() -> PathBuf {
    PathBuf::from("/var/lib/dcontroller/changes.jsonl")
}

fn default_bind() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8080))
}

impl Config {
    /// Parses a config file's text.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let cfg: Self = toml::from_str(text)?;
        if let Some(bad) = cfg.guard_users.iter().find(|u| !valid_user(u)) {
            anyhow::bail!("guard_users: {bad:?} is not a unix user name");
        }
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

/// What the API handlers share.
#[derive(Clone)]
pub struct Shared {
    pub dante: dante::Shared,
    pub changes: std::sync::Arc<change::Changes>,
    pub history: std::sync::Arc<history::Store>,
    pub meters: std::sync::Arc<meters::Watches>,
}

async fn devices(State(s): State<Shared>) -> axum::Json<dcontroller_api::Devices> {
    axum::Json(dante::snapshot(&s.dante))
}

/// The user Cloudflare Access let in (its header); `local` without Access.
/// The server binds loopback only, so only the tunnel sets this header.
fn user(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("cf-access-authenticated-user-email")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .unwrap_or("local")
        .to_owned()
}

async fn subscription(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<dcontroller_api::SubscriptionChange>,
) -> Result<axum::Json<dcontroller_api::ChangeResult>, StatusCode> {
    let who = user(&headers);
    tokio::task::spawn_blocking(move || s.changes.apply(&s.dante, &who, &req))
        .await
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "change: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn multicast(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<dcontroller_api::MulticastCreate>,
) -> Result<axum::Json<dcontroller_api::ChangeResult>, StatusCode> {
    let who = user(&headers);
    tokio::task::spawn_blocking(move || s.changes.multicast(&s.dante, &who, &req))
        .await
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "multicast: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn multicast_delete(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<dcontroller_api::MulticastDelete>,
) -> Result<axum::Json<dcontroller_api::ChangeResult>, StatusCode> {
    let who = user(&headers);
    tokio::task::spawn_blocking(move || s.changes.multicast_delete(&s.dante, &who, &req))
        .await
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "multicast delete: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn tx_flows(
    State(s): State<Shared>,
    axum::extract::Path(device): axum::extract::Path<String>,
) -> Result<axum::Json<dcontroller_api::TxFlows>, (StatusCode, String)> {
    tokio::task::spawn_blocking(move || flows::list(&s.dante, &device))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "flows: task failed");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
        })?
        .map(axum::Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))
}

async fn overview(
    State(s): State<Shared>,
) -> Result<axum::Json<dcontroller_api::Overview>, StatusCode> {
    let now = history::unix_now();
    tokio::task::spawn_blocking(move || s.history.overview(now))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "overview: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "overview: read failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `?from=&to=` of `/api/history`, unix seconds; `to` defaults to now and
/// `from` to an hour before `to`.
#[derive(Debug, Deserialize)]
pub struct Range {
    pub from: Option<i64>,
    pub to: Option<i64>,
}

async fn history_report(
    State(s): State<Shared>,
    axum::extract::Path(device): axum::extract::Path<String>,
    axum::extract::Query(r): axum::extract::Query<Range>,
) -> Result<axum::Json<dcontroller_api::History>, StatusCode> {
    let now = history::unix_now();
    let to = r.to.unwrap_or(now);
    let from = r.from.unwrap_or(to.saturating_sub(3600));
    tokio::task::spawn_blocking(move || s.history.history(&device, from, to, now))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "history: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "history: read failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn meter_report(
    State(s): State<Shared>,
    axum::extract::Path(device): axum::extract::Path<String>,
) -> Result<axum::Json<dcontroller_api::MeterReport>, StatusCode> {
    tokio::task::spawn_blocking(move || s.meters.report(&s.dante, &device))
        .await
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "meters: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `?devices=a,b,…` of `GET /api/levels`.
#[derive(Debug, Deserialize)]
pub struct LevelsQuery {
    #[serde(default)]
    pub devices: String,
}

/// The meter reports of several devices in one answer (the routing
/// matrix's bars): each starts or keeps its device's watch, as
/// `/api/meters/{device}` does.
async fn levels_report(
    State(s): State<Shared>,
    axum::extract::Query(q): axum::extract::Query<LevelsQuery>,
) -> Result<axum::Json<Vec<dcontroller_api::MeterReport>>, StatusCode> {
    let names = meters::names(&q.devices);
    tokio::task::spawn_blocking(move || {
        names
            .iter()
            .map(|n| s.meters.report(&s.dante, n))
            .collect::<Vec<_>>()
    })
    .await
    .map(axum::Json)
    .map_err(|e| {
        tracing::error!(error = %e, "levels: task failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

async fn rename(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<dcontroller_api::Rename>,
) -> Result<axum::Json<dcontroller_api::ChangeResult>, StatusCode> {
    let who = user(&headers);
    tokio::task::spawn_blocking(move || s.changes.rename(&s.dante, &who, &req))
        .await
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "rename: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn setting(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<dcontroller_api::SettingChange>,
) -> Result<axum::Json<dcontroller_api::ChangeResult>, StatusCode> {
    let who = user(&headers);
    tokio::task::spawn_blocking(move || s.changes.setting(&s.dante, &who, &req))
        .await
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "setting: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn changes(
    State(s): State<Shared>,
) -> Result<axum::Json<Vec<dcontroller_api::ChangeEntry>>, StatusCode> {
    s.changes.list().map(axum::Json).map_err(|e| {
        tracing::error!(error = %e, "changes log: read failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

async fn healthz() -> axum::Json<Health> {
    axum::Json(Health {
        status: "ok".into(),
        version: VERSION.into(),
    })
}

/// The whole HTTP app: API routes (unknown `/api/*` is 404), then the PWA
/// from `web_dir` with `index.html` as the fallback for client-side routes.
/// Nothing is cached by the browser, so a deploy shows at once. `shared`
/// holds the Dante state ([`dante::start`]) and the changes log.
pub fn app(web_dir: &Path, shared: Shared) -> Router {
    let index = web_dir.join("index.html");
    let spa = ServeDir::new(web_dir).fallback(ServeFile::new(index));
    Router::new()
        .route("/api/version", get(version))
        .route("/api/devices", get(devices))
        .route("/api/subscription", axum::routing::post(subscription))
        .route("/api/rename", axum::routing::post(rename))
        .route("/api/setting", axum::routing::post(setting))
        .route("/api/changes", get(changes))
        .route("/api/multicast", axum::routing::post(multicast))
        .route(
            "/api/multicast/delete",
            axum::routing::post(multicast_delete),
        )
        .route("/api/flows/{device}", get(tx_flows))
        .route("/api/history/{device}", get(history_report))
        .route("/api/overview", get(overview))
        .route("/api/meters/{device}", get(meter_report))
        .route("/api/levels", get(levels_report))
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*rest}", any(api_not_found))
        .route("/healthz", get(healthz))
        .with_state(shared)
        .fallback_service(spa)
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(TraceLayer::new_for_http())
}
