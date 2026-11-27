//! The video network reader (dcontroller #124 phase 1): camera-box's video
//! nodes (camboxes, the OBS boxes), found through the discovery URL the
//! box's config names (`video_nodes_url`, a site fact), each asked its
//! `GET` every [`READ_EVERY`]. The contract is camera-box's (#1540,
//! schema 1): every connection's counters are cumulative since its
//! `counters_since` within the node's `run_id`; the reader stores what each
//! read added, so a restart or a reconnect counts its new values from zero.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use dcontroller_api as api;
use serde::Deserialize;
use tracing::{info, warn};

/// How often every node is asked (and its connections stored).
pub const READ_EVERY: Duration = Duration::from_secs(10);
/// How often the node list is read again.
pub const DISCOVER_EVERY: Duration = Duration::from_secs(60);
/// How long one ask waits.
pub const WAIT: Duration = Duration::from_secs(2);
/// Unanswered asks in a row after which a node is taken as silent.
pub const MISSES: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Listed {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    pub url: String,
}

#[derive(Deserialize)]
struct Discovery {
    #[serde(default)]
    schema: Option<u64>,
    #[serde(default)]
    nodes: Vec<Listed>,
}

/// One connection as a node's read gives it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ConnRead {
    pub id: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub sink: String,
    #[serde(default)]
    pub sink_input: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub latency_ms: Option<f64>,
    #[serde(default)]
    pub frames_lost: u64,
    #[serde(default)]
    pub frames_duplicated: u64,
    #[serde(default)]
    pub frames_skipped: u64,
    #[serde(default)]
    pub slices_concealed: u64,
    #[serde(default)]
    pub counters_since: String,
}

/// A node's read.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct NodeRead {
    #[serde(default)]
    pub schema: Option<u64>,
    #[serde(default)]
    pub run_id: String,
    #[serde(default)]
    pub connections: Vec<ConnRead>,
}

fn schema_ok(schema: Option<u64>) -> Result<(), String> {
    match schema {
        Some(n) if n > u64::from(api::VIDEO_SCHEMA) => Err(format!(
            "its read is schema {n}, newer than this dcontroller reads ({}): update dcontroller",
            api::VIDEO_SCHEMA
        )),
        _ => Ok(()),
    }
}

/// The node list; a newer schema is refused, not misread.
pub fn parse_discovery(body: &str) -> Result<Vec<Listed>, String> {
    let d: Discovery =
        serde_json::from_str(body).map_err(|e| format!("node list is no JSON: {e}"))?;
    schema_ok(d.schema)?;
    Ok(d.nodes)
}

/// A node's read; a newer schema is refused, not misread.
pub fn parse_node(body: &str) -> Result<NodeRead, String> {
    let n: NodeRead = serde_json::from_str(body).map_err(|e| format!("read is no JSON: {e}"))?;
    schema_ok(n.schema)?;
    Ok(n)
}

/// A connection's counters at a read, with what they count from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    pub run_id: String,
    pub since: String,
    pub lost: u64,
    pub duplicated: u64,
    pub skipped: u64,
    pub concealed: u64,
}

impl Counters {
    pub fn of(run_id: &str, c: &ConnRead) -> Self {
        Counters {
            run_id: run_id.to_owned(),
            since: c.counters_since.clone(),
            lost: c.frames_lost,
            duplicated: c.frames_duplicated,
            skipped: c.frames_skipped,
            concealed: c.slices_concealed,
        }
    }
}

/// What `now` added since `before`: frames lost (concealed slices too),
/// duplicated, skipped. A new run or a reconnect (`counters_since`), or a
/// counter that fell, counts its new value from zero; the first read of a
/// connection adds nothing (what it counted came before dcontroller looked).
pub fn delta(before: Option<&Counters>, now: &Counters) -> (u64, u64, u64) {
    let Some(b) = before else {
        return (0, 0, 0);
    };
    let fresh = b.run_id != now.run_id || b.since != now.since;
    let d = |a: u64, z: u64| if fresh || z < a { z } else { z - a };
    (
        d(b.lost, now.lost) + d(b.concealed, now.concealed),
        d(b.duplicated, now.duplicated),
        d(b.skipped, now.skipped),
    )
}

/// A connection as last read, and when.
#[derive(Debug, Clone)]
pub struct Seen {
    pub node: String,
    pub conn: ConnRead,
    pub at: Instant,
}

/// What the reader last saw: the discovery's error, the nodes with when
/// each last answered, every connection by id.
#[derive(Debug, Default)]
pub struct State {
    pub discovery_error: Option<String>,
    pub nodes: Vec<(api::VideoNode, Option<Instant>)>,
    pub conns: HashMap<String, Seen>,
}

pub type Latest = Arc<RwLock<State>>;

fn ask(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    agent
        .get(url)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("HTTP {code}"),
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
        .map_err(|e| format!("not read: {e}"))
}

/// Starts the reader; nothing runs without a discovery URL.
pub fn start(
    url: Option<String>,
    latest: Latest,
    history: Arc<crate::history::Store>,
    stop: Arc<AtomicBool>,
) {
    let Some(url) = url.filter(|u| !u.trim().is_empty()) else {
        info!("video: no node list configured (`video_nodes_url`), none asked");
        return;
    };
    info!(url = %url, "video: reading the video nodes");
    std::thread::Builder::new()
        .name("video".into())
        .spawn(move || run(&url, &latest, &history, &stop))
        .expect("spawn the video reader");
}

fn run(url: &str, latest: &Latest, history: &crate::history::Store, stop: &AtomicBool) {
    let agent = ureq::AgentBuilder::new().timeout(WAIT).build();
    let mut listed: Vec<Listed> = Vec::new();
    let mut discovered: Option<Instant> = None;
    let mut counters: HashMap<String, Counters> = HashMap::new();
    let mut misses: HashMap<String, u32> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        // An unread list is asked again on the next round, not a minute on.
        let unread = latest
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .discovery_error
            .is_some();
        if unread || discovered.is_none_or(|t| t.elapsed() >= DISCOVER_EVERY) {
            discovered = Some(Instant::now());
            let read = ask(&agent, url).and_then(|b| parse_discovery(&b));
            let mut st = latest.write().unwrap_or_else(|e| e.into_inner());
            match read {
                Ok(nodes) => {
                    if nodes != listed {
                        info!(nodes = nodes.len(), "video: node list read");
                    }
                    st.discovery_error = None;
                    // Nodes keep their last state across a re-read.
                    st.nodes = nodes
                        .iter()
                        .map(|l| {
                            st.nodes
                                .iter()
                                .find(|(n, _)| n.name == l.name)
                                .cloned()
                                .unwrap_or_else(|| {
                                    (
                                        api::VideoNode {
                                            name: l.name.clone(),
                                            kind: l.kind.clone(),
                                            age_s: None,
                                            error: Some("not asked yet".into()),
                                        },
                                        None,
                                    )
                                })
                        })
                        .collect();
                    listed = nodes;
                }
                Err(e) => {
                    if st.discovery_error.as_deref() != Some(e.as_str()) {
                        warn!(url = %url, error = %e, "video: node list not read");
                    }
                    st.discovery_error = Some(e);
                }
            }
        }
        let t = crate::history::unix_now();
        for l in &listed {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let read = ask(&agent, &l.url).and_then(|b| parse_node(&b));
            let mut st = latest.write().unwrap_or_else(|e| e.into_inner());
            let Some(slot) = st.nodes.iter().position(|(n, _)| n.name == l.name) else {
                continue;
            };
            match read {
                Ok(r) => {
                    let was = st.nodes[slot].0.error.take();
                    if was.is_some() {
                        info!(node = %l.name, "video: node answers");
                    }
                    st.nodes[slot].1 = Some(Instant::now());
                    misses.insert(l.name.clone(), 0);
                    for c in r.connections {
                        let now = Counters::of(&r.run_id, &c);
                        let (lost, duplicated, skipped) = delta(counters.get(&c.id), &now);
                        counters.insert(c.id.clone(), now);
                        let p = api::VideoPoint {
                            t,
                            latency_ms: c.latency_ms,
                            lost,
                            duplicated,
                            skipped,
                        };
                        if let Err(e) = history.add_video(&c.id, &p) {
                            warn!(conn = %c.id, error = %e, "video: read not stored");
                        }
                        if lost > 0 {
                            warn!(conn = %c.id, lost, "video: frames lost");
                        }
                        st.conns.insert(
                            c.id.clone(),
                            Seen {
                                node: l.name.clone(),
                                conn: c,
                                at: Instant::now(),
                            },
                        );
                    }
                }
                Err(e) => {
                    let m = misses.entry(l.name.clone()).or_insert(0);
                    *m += 1;
                    let (node, at) = &mut st.nodes[slot];
                    if (at.is_none() || *m >= MISSES) && node.error.as_deref() != Some(e.as_str()) {
                        warn!(node = %l.name, error = %e, "video: node does not answer");
                        node.error = Some(e);
                    }
                }
            }
        }
        let until = Instant::now() + READ_EVERY;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The report `GET /api/video` gives: the nodes and each connection seen,
/// with its windows from the history store.
pub fn report(
    latest: &Latest,
    history: &crate::history::Store,
    now: i64,
) -> Result<api::VideoReport, String> {
    let (discovery_error, nodes, seen): (Option<String>, Vec<api::VideoNode>, Vec<Seen>) = {
        let st = latest.read().unwrap_or_else(|e| e.into_inner());
        (
            st.discovery_error.clone(),
            st.nodes
                .iter()
                .map(|(n, at)| api::VideoNode {
                    age_s: at.map(|a| i64::try_from(a.elapsed().as_secs()).unwrap_or(i64::MAX)),
                    ..n.clone()
                })
                .collect(),
            st.conns.values().cloned().collect(),
        )
    };
    let mut connections = Vec::with_capacity(seen.len());
    for s in seen {
        let w = history.video_windows(&s.conn.id, now)?;
        connections.push(api::VideoConn {
            id: s.conn.id.clone(),
            node: s.node,
            source: s.conn.source,
            sink: s.conn.sink,
            sink_input: s.conn.sink_input,
            kind: s.conn.kind,
            direction: s.conn.direction,
            latency_ms: s.conn.latency_ms,
            lost_1h: w[0].0,
            duplicated_1h: w[0].1,
            skipped_1h: w[0].2,
            lost_3h: w[1].0,
            duplicated_3h: w[1].1,
            skipped_3h: w[1].2,
            lost_5m: w[3].0,
            duplicated_5m: w[3].1,
            skipped_5m: w[3].2,
            lost_24h: w[2].0,
            duplicated_24h: w[2].1,
            skipped_24h: w[2].2,
            spark: history.video_spark(&s.conn.id, now)?,
            age_s: Some(i64::try_from(s.at.elapsed().as_secs()).unwrap_or(i64::MAX)),
        });
    }
    connections.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(api::VideoReport {
        now,
        discovery_error,
        nodes,
        connections,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: &str = r#"{"schema": 1, "node": "strih", "kind": "obs",
        "read_at": "2026-11-20T09:30:05Z", "run_start": "2026-11-20T02:49:10Z",
        "run_id": "boot:1",
        "connections": [{"id": "jxs:cam3->strih:in", "source": "cam3", "sink": "strih",
            "sink_input": "JXS cam3 src", "kind": "jxs", "direction": "in",
            "latency_ms": 41.0, "frames_received": 98765, "frames_lost": 2,
            "frames_duplicated": 4, "frames_skipped": 1, "slices_lost": 0,
            "slices_concealed": 3, "relocks": 0, "decoder_opens": 1,
            "counters_since": "2026-11-20T02:49:12Z", "something_new": true}]}"#;

    #[test]
    fn a_node_list_and_a_nodes_read_parse_and_a_newer_schema_is_refused() {
        let l = parse_discovery(
            r#"{"schema": 1, "venue": "x", "nodes": [{"name": "cam2", "kind": "cambox", "url": "http://h:1/video.json"}]}"#,
        )
        .unwrap();
        assert_eq!(
            l[0],
            Listed {
                name: "cam2".into(),
                kind: "cambox".into(),
                url: "http://h:1/video.json".into()
            }
        );
        let e = parse_discovery(r#"{"schema": 2, "nodes": []}"#).unwrap_err();
        assert!(
            e.contains("schema 2, newer than this dcontroller reads (1)"),
            "{e}"
        );
        let n = parse_node(NODE).unwrap();
        assert_eq!(n.run_id, "boot:1");
        let c = &n.connections[0];
        assert_eq!(
            (c.source.as_str(), c.sink.as_str(), c.kind.as_str()),
            ("cam3", "strih", "jxs")
        );
        assert_eq!(
            (c.latency_ms, c.frames_lost, c.slices_concealed),
            (Some(41.0), 2, 3)
        );
        assert!(parse_node(&NODE.replace(r#""schema": 1"#, r#""schema": 3"#)).is_err());
        assert!(parse_node("not json").is_err());
    }

    /// An OBS box's read as camera-box serves it (strih-snv, 27.11.2026):
    /// a JPEG XS input names its loss in lost slices, a Dante input in
    /// receive holes; neither carries `frames_lost`.
    const OBS: &str = r#"{"schema": 1, "node": "strih-snv", "kind": "obs",
        "read_at": "2026-11-27T15:10:30+01:00", "run_start": "2026-11-27T11:17:16+01:00",
        "run_id": "2026-11-27 11-17-16.txt",
        "connections": [
          {"id": "jxs:strih-snv:JXS cam2 src", "sink": "strih-snv", "sink_input": "JXS cam2 src",
           "kind": "jxs", "direction": "in", "counters_since": "2026-11-27T11:17:22.633+01:00",
           "source": "cam2", "jxs_frames": 419601, "jxs_lost_slices": 61, "jxs_concealed": 1,
           "received": 419567, "underruns": 100, "relocks": 0, "latency_ms": 36,
           "frames_received": 419567, "frames_duplicated": 100, "slices_lost": 61,
           "slices_concealed": 1},
          {"id": "dante:strih-snv:mix", "sink": "strih-snv", "sink_input": "mix",
           "kind": "dante", "direction": "in", "counters_since": "2026-11-27T11:17:22.700+01:00",
           "rx_packets": 1000, "rx_gaps": 2, "rx_holes": 5, "rx_loss": 0.5}]}"#;

    #[test]
    fn an_obs_read_counts_lost_slices_and_dante_holes_as_lost() {
        let n = parse_node(OBS).unwrap();
        let jxs = Counters::of(&n.run_id, &n.connections[0]);
        assert_eq!((jxs.lost, jxs.duplicated, jxs.concealed), (61, 100, 1));
        assert_eq!(n.connections[0].latency_ms, Some(36.0));
        let dante = Counters::of(&n.run_id, &n.connections[1]);
        assert_eq!(dante.lost, 5);
    }

    #[test]
    fn a_reads_frames_are_what_it_added_and_a_restart_counts_from_zero() {
        let c = |run: &str, since: &str, lost, dup, skip, conc| Counters {
            run_id: run.into(),
            since: since.into(),
            lost,
            duplicated: dup,
            skipped: skip,
            concealed: conc,
        };
        let a = c("r1", "s1", 2, 4, 1, 3);
        // The first read: what it counted came before.
        assert_eq!(delta(None, &a), (0, 0, 0));
        assert_eq!(delta(Some(&a), &c("r1", "s1", 5, 4, 2, 3)), (3, 0, 1));
        // Concealed slices are a loss too.
        assert_eq!(delta(Some(&a), &c("r1", "s1", 2, 4, 1, 7)), (4, 0, 0));
        // A node restart, a reconnect, or a counter that fell: from zero.
        assert_eq!(delta(Some(&a), &c("r2", "s1", 1, 0, 0, 0)), (1, 0, 0));
        assert_eq!(delta(Some(&a), &c("r1", "s2", 0, 2, 0, 0)), (0, 2, 0));
        assert_eq!(delta(Some(&a), &c("r1", "s1", 1, 4, 1, 3)), (1, 0, 0));
    }
}
