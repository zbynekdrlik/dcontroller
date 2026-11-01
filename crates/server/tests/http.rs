use axum::body::Body;
use axum::http::{Request, StatusCode};
use dcontroller::{Config, Health, VERSION, app};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn off() -> dcontroller::Shared {
    let log = tempfile::NamedTempFile::new()
        .unwrap()
        .into_temp_path()
        .to_path_buf();
    dcontroller::Shared {
        dante: std::sync::Arc::new(std::sync::RwLock::new(dcontroller::dante::State::off(
            None, None,
        ))),
        changes: std::sync::Arc::new(dcontroller::change::Changes::new(log)),
    }
}

fn web() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("index.html"), "<html>shell</html>").unwrap();
    std::fs::write(d.path().join("manifest.webmanifest"), "{\"name\":\"x\"}").unwrap();
    d
}

async fn get(dir: &std::path::Path, uri: &str) -> (StatusCode, String, Option<String>) {
    let res = app(dir, off())
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
    let (s, body, cache) = get(d.path(), "/").await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "<html>shell</html>"));
    assert_eq!(cache.as_deref(), Some("no-store"));
    let (s, body, _) = get(d.path(), "/devices/some-route").await;
    assert_eq!((s, body.as_str()), (StatusCode::OK, "<html>shell</html>"));
}

#[test]
fn config_defaults_and_rejects_unknown_keys() {
    let c = Config::parse("web_dir = \"/srv/web\"").unwrap();
    assert_eq!(c.bind.to_string(), "127.0.0.1:8080");
    assert_eq!(c.dante_interface, None);
    let c = Config::parse("web_dir = \"d\"\nbind = \"127.0.0.1:9000\"\ndante_interface = \"eth1\"")
        .unwrap();
    assert_eq!(c.bind.to_string(), "127.0.0.1:9000");
    assert_eq!(c.dante_interface.as_deref(), Some("eth1"));
    assert!(Config::parse("web_dir = \"d\"\ntypo = 1").is_err());
    assert!(Config::parse("bind = \"127.0.0.1:1\"").is_err());
}

#[tokio::test]
async fn unknown_api_path_is_404_not_the_shell() {
    let d = web();
    let (s, body, _) = get(d.path(), "/api/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(!body.contains("shell"));
    for uri in ["/api/a/b", "/api", "/api/"] {
        let (s, body, _) = get(d.path(), uri).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        assert!(!body.contains("shell"), "{uri}");
    }
}

#[test]
fn public_bind_needs_allow_remote() {
    let e = Config::parse("web_dir = \"d\"\nbind = \"0.0.0.0:9000\"").unwrap_err();
    assert!(e.to_string().contains("not loopback"), "{e}");
    let c = Config::parse("web_dir = \"d\"\nbind = \"0.0.0.0:9000\"\nallow_remote = true").unwrap();
    assert!(c.allow_remote);
    assert!(Config::parse("web_dir = \"d\"\nbind = \"[::1]:9000\"").is_ok());
}

#[test]
fn validate_requires_built_web_dir() {
    let empty = tempfile::tempdir().unwrap();
    let c = Config::parse(&format!("web_dir = {:?}", empty.path())).unwrap();
    let e = c.validate().unwrap_err();
    assert!(e.to_string().contains("no index.html"), "{e}");
    let d = web();
    let c = Config::parse(&format!("web_dir = {:?}", d.path())).unwrap();
    c.validate().unwrap();
}

#[tokio::test]
async fn api_devices_without_a_dante_interface_is_an_empty_off_list() {
    let dir = web();
    let (status, body, cache) = get(dir.path(), "/api/devices").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache.as_deref(), Some("no-store"));
    let d: dcontroller_api::Devices = serde_json::from_str(&body).unwrap();
    assert!(!d.enabled);
    assert!(d.devices.is_empty());
    assert_eq!(d.interface, None);
}

async fn post(dir: &std::path::Path, uri: &str, ctype: &str, body: &str) -> (StatusCode, String) {
    let res = app(dir, off())
        .oneshot(
            Request::post(uri)
                .header("content-type", ctype)
                .header("cf-access-authenticated-user-email", "op@example.com")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn a_subscription_change_needs_json_and_a_listed_device() {
    let dir = web();
    let body =
        r#"{"rx_device":"stage","rx_channel":1,"source":{"channel":"Mix L","device":"desk"}}"#;
    // A form post (what a foreign page could send without CORS) is refused.
    let (status, _) = post(dir.path(), "/api/subscription", "text/plain", body).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (status, text) = post(dir.path(), "/api/subscription", "application/json", body).await;
    assert_eq!(status, StatusCode::OK);
    let r: dcontroller_api::ChangeResult = serde_json::from_str(&text).unwrap();
    assert!(!r.ok);
    assert!(r.message.contains("not listed"), "{}", r.message);
}

#[tokio::test]
async fn api_changes_is_a_json_list() {
    let dir = web();
    let (status, body, cache) = get(dir.path(), "/api/changes").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cache.as_deref(), Some("no-store"));
    let list: Vec<dcontroller_api::ChangeEntry> = serde_json::from_str(&body).unwrap();
    assert!(list.is_empty());
}
