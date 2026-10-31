use axum::body::Body;
use axum::http::{Request, StatusCode};
use dcontroller::{Config, Health, VERSION, app};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn web() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("index.html"), "<html>shell</html>").unwrap();
    std::fs::write(d.path().join("manifest.webmanifest"), "{\"name\":\"x\"}").unwrap();
    d
}

async fn get(dir: &std::path::Path, uri: &str) -> (StatusCode, String, Option<String>) {
    let res = app(dir)
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let cache = res
        .headers()
        .get("cache-control")
        .map(|v| v.to_str().unwrap().to_owned());
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap(), cache)
}

#[test]
fn version_is_build_injected_semver() {
    let re = |s: &str| {
        let s = s.strip_prefix('v').expect("starts with v");
        let core = s.split([' ', '-']).next().unwrap();
        core.split('.').count() == 3 && core.split('.').all(|p| p.parse::<u32>().is_ok())
    };
    assert!(re(VERSION), "bad version {VERSION}");
    assert!(VERSION.starts_with(&format!("v{}", env!("CARGO_PKG_VERSION"))));
}

#[tokio::test]
async fn api_version_returns_build_version_uncached() {
    let d = web();
    let (s, body, cache) = get(d.path(), "/api/version").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, VERSION);
    assert_eq!(cache.as_deref(), Some("no-store"));
}

#[tokio::test]
async fn healthz_reports_ok_and_version() {
    let d = web();
    let (s, body, _) = get(d.path(), "/healthz").await;
    assert_eq!(s, StatusCode::OK);
    let h: Health = serde_json::from_str(&body).unwrap();
    assert_eq!(
        h,
        Health {
            status: "ok".into(),
            version: VERSION.into()
        }
    );
}

#[tokio::test]
async fn serves_static_files_and_spa_fallback() {
    let d = web();
    let (s, body, _) = get(d.path(), "/manifest.webmanifest").await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "{\"name\":\"x\"}"));
    let (s, body, _) = get(d.path(), "/").await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "<html>shell</html>"));
    let (s, body, _) = get(d.path(), "/devices/some-route").await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "<html>shell</html>"));
}

#[test]
fn config_defaults_and_rejects_unknown_keys() {
    let c = Config::parse("web_dir = \"/srv/web\"").unwrap();
    assert_eq!(c.bind.to_string(), "127.0.0.1:8080");
    assert_eq!(c.dante_interface, None);
    let c = Config::parse("web_dir = \"d\"\nbind = \"0.0.0.0:9000\"\ndante_interface = \"eth1\"")
        .unwrap();
    assert_eq!(c.bind.to_string(), "0.0.0.0:9000");
    assert_eq!(c.dante_interface.as_deref(), Some("eth1"));
    assert!(Config::parse("web_dir = \"d\"\ntypo = 1").is_err());
    assert!(Config::parse("bind = \"0.0.0.0:1\"").is_err());
}
