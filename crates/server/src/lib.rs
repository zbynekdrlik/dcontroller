//! dcontroller server: serves the built PWA and a small JSON API.

pub mod bridge;
pub mod change;
pub mod clocks;
pub mod dante;
pub mod dsync;
pub mod flows;
pub mod history;
pub mod latency;
pub mod mdns;
pub mod meters;
pub mod video;

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
    /// The PCs' dsync clock services to watch, `host` or `host:port`
    /// (dsync's 8898), asked over the LAN (dcontroller #124). Site facts:
    /// they live in the box's config, not in the repo.
    #[serde(default)]
    pub dsync_nodes: Vec<String>,
    /// camera-box's video node list (`…/video-nodes.json`, dcontroller #124
    /// phase 1). A site fact: it lives in the box's config. Unset = no
    /// video network read.
    #[serde(default)]
    pub video_nodes_url: Option<String>,
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
    pub mdns: std::sync::Arc<mdns::Watch>,
    /// Every dstack device's newest clock figures (#124).
    pub clocks: clocks::Latest,
    /// Every configured dsync service node's newest status (#124).
    pub dsync: dsync::Latest,
    pub video: video::Latest,
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

async fn clocks_now(State(s): State<Shared>) -> axum::Json<dcontroller_api::Clocks> {
    let now = std::time::Instant::now();
    axum::Json(dcontroller_api::Clocks {
        services: dsync::snapshot(&s.dsync, now),
        ..clocks::snapshot(&s.clocks, now)
    })
}

/// A page or thing opened (#142): kept so the design can follow use.
async fn record_use(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    axum::Json(e): axum::Json<dcontroller_api::UseEvent>,
) -> StatusCode {
    let who = user(&headers);
    let now = history::unix_now();
    tracing::info!(user = %who, view = %e.view, target = %e.target, "opened");
    match tokio::task::spawn_blocking(move || s.history.record_use(now, &who, &e)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT,
        Ok(Err(err)) => {
            tracing::error!(error = %err, "usage: not recorded");
            StatusCode::INTERNAL_SERVER_ERROR
        }
        Err(err) => {
            tracing::error!(error = %err, "usage: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

async fn usage(State(s): State<Shared>) -> Result<axum::Json<dcontroller_api::Usage>, StatusCode> {
    let now = history::unix_now();
    tokio::task::spawn_blocking(move || s.history.usage(now))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "usage: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "usage: read failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

async fn links(State(s): State<Shared>) -> Result<axum::Json<dcontroller_api::Links>, StatusCode> {
    let now = history::unix_now();
    tokio::task::spawn_blocking(move || s.history.links(now))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "links: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "links: read failed");
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

/// A bridge device's own figures over a range (dcontroller #108).
async fn bridge_report(
    State(s): State<Shared>,
    axum::extract::Path(device): axum::extract::Path<String>,
    axum::extract::Query(r): axum::extract::Query<Range>,
) -> Result<axum::Json<dcontroller_api::Bridge>, StatusCode> {
    let now = history::unix_now();
    let to = r.to.unwrap_or(now);
    let from = r.from.unwrap_or(to.saturating_sub(3600));
    tokio::task::spawn_blocking(move || s.history.bridge(&device, from, to))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "bridge: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "bridge: read failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// The video network now (#124 phase 1): the nodes and every connection
/// with its windows.
async fn video_now(
    State(s): State<Shared>,
) -> Result<axum::Json<dcontroller_api::VideoReport>, StatusCode> {
    let now = history::unix_now();
    tokio::task::spawn_blocking(move || video::report(&s.video, &s.history, now))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "video: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "video: read failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// A video connection's reads over a range (#124 phase 1).
async fn video_report(
    State(s): State<Shared>,
    axum::extract::Path(conn): axum::extract::Path<String>,
    axum::extract::Query(r): axum::extract::Query<Range>,
) -> Result<axum::Json<dcontroller_api::VideoHistory>, StatusCode> {
    let now = history::unix_now();
    let to = r.to.unwrap_or(now);
    let from = r.from.unwrap_or(to.saturating_sub(3600));
    tokio::task::spawn_blocking(move || s.history.video(&conn, from, to))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "video history: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "video history: read failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// A dsync service node's states over a range (#124 2c), the node as the
/// box's config names it (`host:port`).
async fn dsync_report(
    State(s): State<Shared>,
    axum::extract::Path(node): axum::extract::Path<String>,
    axum::extract::Query(r): axum::extract::Query<Range>,
) -> Result<axum::Json<dcontroller_api::DsyncHistory>, StatusCode> {
    let now = history::unix_now();
    let to = r.to.unwrap_or(now);
    let from = r.from.unwrap_or(to.saturating_sub(3600));
    tokio::task::spawn_blocking(move || s.history.dsync(&node, from, to))
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "dsync history: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "dsync history: read failed");
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

/// What is wrong with a place asked for, if anything.
pub fn validate_place(req: &dcontroller_api::Place) -> Result<(), String> {
    let bad = |s: &str| s.chars().any(char::is_control);
    if req.device.is_empty() || req.device.chars().count() > 64 || bad(&req.device) {
        return Err("not a device name".to_owned());
    }
    if req
        .before
        .as_ref()
        .is_some_and(|b| b.chars().count() > 64 || bad(b))
    {
        return Err("not a device name to stand before".to_owned());
    }
    if req.place != req.place.trim() || bad(&req.place) {
        return Err("a place is plain text with no space at its ends".to_owned());
    }
    if req.place.chars().count() > dcontroller_api::PLACE_LEN {
        return Err(format!(
            "a place has at most {} characters",
            dcontroller_api::PLACE_LEN
        ));
    }
    Ok(())
}

/// `GET /api/places`: where the owner put each device on the Map (#122).
async fn places(
    State(s): State<Shared>,
) -> Result<axum::Json<dcontroller_api::Places>, StatusCode> {
    tokio::task::spawn_blocking(move || s.history.places())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "places: task failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(axum::Json)
        .map_err(|e| {
            tracing::error!(error = %e, "places: not read");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

/// `POST /api/place`: puts a device in a place (empty: in none), and logs
/// it with the changes.
async fn place(
    State(s): State<Shared>,
    headers: axum::http::HeaderMap,
    axum::Json(req): axum::Json<dcontroller_api::Place>,
) -> Result<axum::Json<dcontroller_api::Places>, (StatusCode, String)> {
    let who = user(&headers);
    validate_place(&req).map_err(|e| {
        tracing::warn!(user = %who, device = %req.device, place = %req.place, error = %e, "place: refused");
        (StatusCode::BAD_REQUEST, e)
    })?;
    tokio::task::spawn_blocking(move || {
        let done = s
            .history
            .set_place(&req.device, &req.place, req.before.as_deref());
        let asked = (!req.place.is_empty()).then(|| req.place.clone());
        s.changes.log_made(&dcontroller_api::ChangeEntry {
            at: u64::try_from(history::unix_now()).unwrap_or(0),
            user: who,
            kind: "place".to_owned(),
            what: "place".to_owned(),
            rx_device: req.device.clone(),
            name_before: done.as_ref().ok().and_then(Clone::clone),
            name_asked: asked.clone(),
            name_after: if done.is_ok() { asked } else { None },
            ok: done.is_ok(),
            message: match &done {
                Ok(_) => "place on the Map set".to_owned(),
                Err(e) => format!("not stored: {e}"),
            },
            ..dcontroller_api::ChangeEntry::default()
        });
        done.and_then(|_| s.history.places())
    })
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "place: task failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "task failed".to_owned())
    })?
    .map(axum::Json)
    .map_err(|e| {
        tracing::error!(error = %e, "place: not stored");
        (StatusCode::INTERNAL_SERVER_ERROR, e)
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

/// `GET /api/mdns`: the mDNS senders on the Dante leg (the last minute).
async fn mdns_report(State(s): State<Shared>) -> axum::Json<dcontroller_api::Mdns> {
    axum::Json(s.mdns.view())
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
        .route("/api/bridge/{device}", get(bridge_report))
        .route("/api/dsync/{node}", get(dsync_report))
        .route("/api/video", get(video_now))
        .route("/api/video/{conn}", get(video_report))
        .route("/api/overview", get(overview))
        .route("/api/links", get(links))
        .route("/api/places", get(places))
        .route("/api/place", axum::routing::post(place))
        .route("/api/usage", get(usage).post(record_use))
        .route("/api/clocks", get(clocks_now))
        .route("/api/meters/{device}", get(meter_report))
        .route("/api/levels", get(levels_report))
        .route("/api/mdns", get(mdns_report))
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
