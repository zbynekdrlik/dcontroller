//! The dsync service reader (dcontroller #124): the PCs' dsync clock
//! services, named in the box's config (`dsync_nodes`, site facts stay on
//! the box), asked `GET /status` over the LAN every [`READ_EVERY`] and kept
//! as the newest answer per node for `GET /api/clocks`.
//!
//! dsync's contract is additive: only the health fields it names are read
//! (phase error, lock, mode, the last PTP packet's age, the grandmaster,
//! drift, the clock alarm, the fallback role). `offset_ns` and `settled`
//! are never read: dsync says they are no measure of health.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use dcontroller_api as api;
use serde_json::Value;
use tracing::{info, warn};

/// How often every node is asked.
pub const READ_EVERY: Duration = Duration::from_secs(5);
/// How long one ask waits.
pub const WAIT: Duration = Duration::from_secs(2);
/// Unanswered asks in a row after which an answering node is taken as
/// silent (one lost ask is no outage).
pub const MISSES: u32 = 3;
/// dsync's status port, when the config names a host alone.
pub const PORT: u16 = 8898;

/// The newest read per configured node, in the config's order, and when
/// it last answered.
pub type Latest = Arc<RwLock<Vec<(api::DsyncNode, Option<Instant>)>>>;

/// `name` as `host:port` (dsync's port when it names none).
pub fn address(name: &str) -> String {
    let name = name.trim();
    if name
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        name.to_owned()
    } else {
        format!("{name}:{PORT}")
    }
}

/// A grandmaster id as dsync gives it (six bytes, or text) as a MAC.
fn gm(v: &Value) -> Option<String> {
    match v {
        Value::Array(bytes) if !bytes.is_empty() => Some(
            bytes
                .iter()
                .map(|b| format!("{:02x}", b.as_u64().unwrap_or(0) & 0xff))
                .collect::<Vec<_>>()
                .join(":"),
        ),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// A status body as the API gives it; `Err` when it is no JSON object.
pub fn parse(node: &str, body: &str) -> Result<api::DsyncNode, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("status is no JSON: {e}"))?;
    let o = v.as_object().ok_or("status is no JSON object")?;
    let text = |k: &str| match o.get(k) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    let num = |k: &str| o.get(k).and_then(Value::as_f64);
    let alarm = o
        .get("clock_alarm")
        .and_then(Value::as_object)
        .and_then(|a| {
            a.get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                .then(|| {
                    a.get("reason")
                        .and_then(Value::as_str)
                        .filter(|r| !r.is_empty())
                        .unwrap_or("active, no reason given")
                        .to_owned()
                })
        });
    Ok(api::DsyncNode {
        node: node.to_owned(),
        age_s: Some(0),
        error: None,
        version: text("version"),
        mode: text("mode"),
        ptp_phase_locked: o.get("ptp_phase_locked").and_then(Value::as_bool),
        ptp_phase_error_us: num("ptp_phase_error_us"),
        last_ptp_rx_age_s: num("last_ptp_rx_age_s"),
        ptp_rx_pps: num("ptp_rx_pps"),
        gm_uuid: o.get("gm_uuid").and_then(gm),
        gm_source_ip: text("gm_source_ip"),
        drift_ppm: num("drift_ppm"),
        fallback_role: text("fallback_role"),
        alarm,
    })
}

/// One ask: the node's status, or why there is none.
fn ask(agent: &ureq::Agent, node: &str) -> Result<api::DsyncNode, String> {
    let body = agent
        .get(&format!("http://{node}/status"))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("HTTP {code}"),
            // Without the URL (the node is named next to it), with the
            // cause under it: "Network Error" alone says nothing.
            ureq::Error::Transport(t) => {
                let cause = std::error::Error::source(&t).map(|s| format!(": {s}"));
                format!(
                    "{}{}{}",
                    t.kind(),
                    t.message().map_or_else(String::new, |m| format!(": {m}")),
                    cause.unwrap_or_default()
                )
            }
        })?
        .into_string()
        .map_err(|e| format!("status not read: {e}"))?;
    parse(node, &body)
}

/// Every configured node, each with its answer's age. A node that stopped
/// answering keeps its last answer's fields with the error on top.
pub fn snapshot(latest: &Latest, now: Instant) -> Vec<api::DsyncNode> {
    latest
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(n, at)| api::DsyncNode {
            age_s: at.map(|t| {
                i64::try_from(now.saturating_duration_since(t).as_secs()).unwrap_or(i64::MAX)
            }),
            ..n.clone()
        })
        .collect()
}

/// Starts the reader thread; nothing runs without configured nodes.
pub fn start(nodes: &[String], latest: Latest, stop: Arc<AtomicBool>) {
    let nodes: Vec<String> = nodes.iter().map(|n| address(n)).collect();
    if nodes.is_empty() {
        info!("dsync: no service node configured (`dsync_nodes`), none asked");
        return;
    }
    info!(nodes = ?nodes, "dsync: asking the service nodes");
    *latest.write().unwrap_or_else(|e| e.into_inner()) = nodes
        .iter()
        .map(|n| {
            (
                api::DsyncNode {
                    node: n.clone(),
                    error: Some("not asked yet".to_owned()),
                    ..api::DsyncNode::default()
                },
                None,
            )
        })
        .collect();
    std::thread::Builder::new()
        .name("dsync".into())
        .spawn(move || run(&nodes, &latest, &stop))
        .expect("spawn the dsync reader");
}

/// The HTTP client of the reader.
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(WAIT).build()
}

/// Whether a node's state changed between two reads: worth a log line.
pub fn turned(before: &api::DsyncNode, after: &api::DsyncNode) -> bool {
    // The state, not its wording: the reason carries the phase error,
    // which moves on every read.
    before.health().0 != after.health().0
}

/// Takes one ask's outcome into a node's slot: an answer replaces it; a
/// failed ask leaves the last answer's fields, and after [`MISSES`] in a
/// row puts the error over them.
pub fn take(
    slot: &mut api::DsyncNode,
    at: &mut Option<Instant>,
    misses: &mut u32,
    read: Result<api::DsyncNode, String>,
    now: Instant,
) {
    match read {
        Ok(n) => {
            *slot = n;
            *at = Some(now);
            *misses = 0;
        }
        Err(e) => {
            *misses += 1;
            // One lost ask is no outage: an answering node turns silent
            // after [`MISSES`] in a row; one that never answered says why
            // at once.
            if at.is_none() || *misses >= MISSES {
                slot.error = Some(e);
            }
        }
    }
}

fn run(nodes: &[String], latest: &Latest, stop: &AtomicBool) {
    let agent = agent();
    let mut misses = vec![0u32; nodes.len()];
    while !stop.load(Ordering::Relaxed) {
        for (i, node) in nodes.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let read = ask(&agent, node);
            let mut all = latest.write().unwrap_or_else(|e| e.into_inner());
            let Some((slot, at)) = all.get_mut(i) else {
                continue;
            };
            let was = slot.clone();
            take(slot, at, &mut misses[i], read, Instant::now());
            let after = slot.health();
            // A change of state is the event worth a line.
            if turned(&was, slot) {
                if after.0 == "ok" {
                    info!(node = %node, state = after.0, why = %after.1, "dsync: node state changed");
                } else {
                    warn!(node = %node, state = after.0, why = %after.1, "dsync: node state changed");
                }
            }
        }
        let until = Instant::now() + READ_EVERY;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"{
        "schema": 1, "version": "0.9.4", "mode": "LOCK",
        "ptp_phase_error_us": 3.25, "ptp_phase_locked": true,
        "last_ptp_rx_age_s": 0.4, "ptp_rx_pps": 8.0,
        "gm_uuid": [0, 29, 193, 170, 187, 204], "gm_source_ip": "10.0.7.200",
        "drift_ppm": -1.75, "fallback_role": "none",
        "clock_alarm": {"active": false, "since": null, "reason": ""},
        "offset_ns": 123456789012, "settled": true, "something_new": [1, 2]
    }"#;

    #[test]
    fn a_status_reads_its_health_fields_and_nothing_else() {
        let n = parse("pc:8898", STATUS).unwrap();
        assert_eq!(n.node, "pc:8898");
        assert_eq!(
            (n.version.as_deref(), n.mode.as_deref()),
            (Some("0.9.4"), Some("LOCK"))
        );
        assert_eq!(
            (n.ptp_phase_locked, n.ptp_phase_error_us),
            (Some(true), Some(3.25))
        );
        assert_eq!((n.last_ptp_rx_age_s, n.ptp_rx_pps), (Some(0.4), Some(8.0)));
        assert_eq!(n.gm_uuid.as_deref(), Some("00:1d:c1:aa:bb:cc"));
        assert_eq!(n.gm_source_ip.as_deref(), Some("10.0.7.200"));
        assert_eq!(
            (n.drift_ppm, n.fallback_role.as_deref()),
            (Some(-1.75), Some("none"))
        );
        assert_eq!((n.alarm.clone(), n.error.clone()), (None, None));
        assert_eq!(n.health().0, "ok");
    }

    #[test]
    fn an_active_alarm_a_never_heard_ptp_and_a_bare_answer_read_as_they_are() {
        let n = parse(
            "pc:8898",
            r#"{"mode": "FALLBACK", "ptp_phase_locked": false, "last_ptp_rx_age_s": null,
                "gm_uuid": "", "clock_alarm": {"active": true, "since": "2026-11-19T01:00:00Z", "reason": "no PTP for 10 s"}}"#,
        )
        .unwrap();
        assert_eq!(n.alarm.as_deref(), Some("no PTP for 10 s"));
        assert_eq!((n.last_ptp_rx_age_s, n.gm_uuid.clone()), (None, None));
        assert_eq!(
            n.health(),
            ("bad", "clock alarm: no PTP for 10 s".to_owned())
        );
        // An answer with none of the fields is still an answer.
        let bare = parse("pc:8898", "{}").unwrap();
        assert_eq!(bare.health().0, "warn");
        assert!(parse("pc:8898", "<html>").is_err());
        assert!(parse("pc:8898", "[1]").is_err());
    }

    /// A node as dsync's own server behaves on some PCs: it answers one
    /// request and closes the connection without saying so.
    fn closing_server(answers: usize) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for _ in 0..answers {
                let Ok((mut c, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0u8; 2048];
                let _ = c.read(&mut buf);
                let body = STATUS.as_bytes();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                );
                let _ = c.write_all(head.as_bytes());
                let _ = c.write_all(body);
                // Closed here, with no `Connection: close` sent.
            }
        });
        addr
    }

    #[test]
    fn a_node_that_closes_after_each_answer_is_read_every_time() {
        // Seen live: a node answering curl at once read "Network Error" on
        // every other ask (a kept connection the node had closed).
        let node = closing_server(3);
        let agent = agent();
        for round in 1..=3 {
            let n = ask(&agent, &node).unwrap_or_else(|e| panic!("ask {round}: {e}"));
            assert_eq!(n.mode.as_deref(), Some("LOCK"), "ask {round}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn one_unanswered_ask_does_not_make_an_answering_node_silent() {
        // Seen live: about one ask in twenty to a healthy node fails with a
        // network error (the node serves one client at a time), and each
        // one turned the node red for five seconds.
        let now = Instant::now();
        let mut slot = parse("pc:8898", STATUS).unwrap();
        let (mut at, mut misses) = (Some(now), 0);
        for _ in 1..MISSES {
            take(
                &mut slot,
                &mut at,
                &mut misses,
                Err("Network Error".into()),
                now,
            );
            assert_eq!(slot.error, None, "still taken as answering");
            assert_eq!(slot.health().0, "ok");
        }
        take(
            &mut slot,
            &mut at,
            &mut misses,
            Err("Network Error".into()),
            now,
        );
        assert_eq!(slot.error.as_deref(), Some("Network Error"));
        // An answer clears it and the count.
        take(
            &mut slot,
            &mut at,
            &mut misses,
            parse("pc:8898", STATUS),
            now,
        );
        assert_eq!((slot.error.clone(), misses), (None, 0));
        // A node that never answered says why at once.
        let mut fresh = api::DsyncNode {
            node: "new:8898".into(),
            error: Some("not asked yet".into()),
            ..api::DsyncNode::default()
        };
        let (mut at, mut misses) = (None, 0);
        take(
            &mut fresh,
            &mut at,
            &mut misses,
            Err("connection refused".into()),
            now,
        );
        assert_eq!(fresh.error.as_deref(), Some("connection refused"));
    }

    #[test]
    fn a_new_reading_of_the_same_state_is_no_change() {
        let a = parse("pc:8898", STATUS).unwrap();
        let mut b = a.clone();
        b.ptp_phase_error_us = Some(-0.4);
        // The phase error moves on every read: the state did not.
        assert!(!turned(&a, &b));
        b.ptp_phase_locked = Some(false);
        assert!(turned(&a, &b));
        let mut gone = a.clone();
        gone.error = Some("timed out".into());
        assert!(turned(&a, &gone));
        // Still not answering, for another reason: still bad, no new line.
        let mut still = gone.clone();
        still.error = Some("connection refused".into());
        assert!(!turned(&gone, &still));
    }

    #[test]
    fn a_node_named_without_a_port_gets_dsyncs() {
        assert_eq!(address("mixer.lan"), "mixer.lan:8898");
        assert_eq!(address(" 10.0.9.5:9000 "), "10.0.9.5:9000");
        assert_eq!(address("10.0.9.5"), "10.0.9.5:8898");
    }

    #[test]
    fn a_node_that_stopped_answering_keeps_its_last_fields_under_the_error() {
        let latest: Latest = Latest::default();
        let now = Instant::now();
        let mut n = parse("pc:8898", STATUS).unwrap();
        n.error = Some("timed out".into());
        *latest.write().unwrap() = vec![
            (n, Some(now - Duration::from_secs(42))),
            (
                api::DsyncNode {
                    node: "new:8898".into(),
                    error: Some("not asked yet".into()),
                    ..api::DsyncNode::default()
                },
                None,
            ),
        ];
        let s = snapshot(&latest, now);
        assert_eq!((s[0].age_s, s[0].mode.as_deref()), (Some(42), Some("LOCK")));
        assert_eq!(s[0].health(), ("bad", "no answer: timed out".to_owned()));
        assert_eq!((s[1].age_s, s[1].health().0), (None, "bad"));
    }
}
