//! The clock reader (dcontroller #124): every dstack device's own clock
//! figures (dsync's follower in the device, and its poll of the PC's dsync
//! service), asked with dstack's clock figures request on the Dante leg
//! (dstack #429, `dstack::info::client::clock_figures`) and kept as the
//! newest read per device for `GET /api/clocks`.
//!
//! Only a dstack device from v0.6.187 on answers. Every listed device is
//! asked; one that does not answer is asked again only after
//! [`RETRY_QUIET`], so the reader sends a request every [`READ_EVERY`] per
//! answering device and a few an hour per other device.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use dcontroller_api as api;
use dstack::info::ClockFigures;
use dstack::info::client::{self, InfoError};
use dstack::info::clock_figures::{TimeSource, poll_code, waiting_code};
use tracing::{info, warn};

use crate::dante::Shared;
use crate::history::unix_now;

/// How often an answering device is asked.
pub const READ_EVERY: Duration = Duration::from_secs(2);
/// How long one request waits for its reply.
pub const WAIT: Duration = Duration::from_millis(300);
/// How long a device that did not answer waits before it is asked again.
pub const RETRY_QUIET: Duration = Duration::from_secs(300);
/// Unanswered reads in a row after which a device is taken as gone quiet.
pub const MISSES: u32 = 5;
/// A read older than this is no longer listed, seconds.
pub const STALE_S: u64 = 60;

/// The newest read per device (by its name in lower case), and when.
pub type Latest = Arc<RwLock<HashMap<String, (api::ClockNode, Instant)>>>;

fn mac(id: [u8; 6]) -> String {
    id.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// One read as the API gives it.
pub fn node(device: &str, f: &ClockFigures) -> api::ClockNode {
    api::ClockNode {
        device: device.to_owned(),
        age_s: 0,
        source: match f.source {
            TimeSource::None => "none",
            TimeSource::Internal => "follower",
            TimeSource::Service => "service",
            TimeSource::System => "system",
        }
        .to_owned(),
        ready: f.ready,
        follower: f.follower.as_ref().map(|x| api::Follower {
            locked: x.locked,
            settled: x.settled,
            holdover: x.holdover,
            waiting: match waiting_code(x.waiting) {
                0 => None,
                1 => Some("stopped"),
                2 => Some("no PTP heard"),
                3 => Some("not locked"),
                4 => Some("not settled"),
                _ => Some("waits to agree"),
            }
            .map(str::to_owned),
            grandmaster: x.grandmaster.map(mac),
            holdovers: x.holdovers,
            freq_ppb: x.freq_ppb,
            error_p90_ns: x.error_p90_ns,
        }),
        service: f.service.as_ref().map(|x| api::ServicePoll {
            poll: match poll_code(Some(x.poll)) {
                1 => "usable",
                2 => "no reply",
                3 => "not locked",
                4 => "no date state",
                5 => "step near",
                6 => "slewing",
                _ => "none",
            }
            .to_owned(),
            reply_age_ms: x.reply_age_ms,
            locked: x.locked,
            settled: x.settled,
            holdover: x.holdover,
            holdovers: x.holdovers,
            freq_ppb: x.freq_ppb,
            grandmaster: x.grandmaster.map(mac),
        }),
    }
}

/// Every device read within [`STALE_S`], by name, each with its read's age.
pub fn snapshot(latest: &Latest, now: Instant) -> api::Clocks {
    let map = latest.read().unwrap_or_else(|e| e.into_inner());
    let mut nodes: Vec<api::ClockNode> = map
        .values()
        .filter_map(|(n, at)| {
            let age = now.saturating_duration_since(*at).as_secs();
            (age <= STALE_S).then(|| api::ClockNode {
                age_s: i64::try_from(age).unwrap_or(i64::MAX),
                ..n.clone()
            })
        })
        .collect();
    nodes.sort_by_key(|n| n.device.to_lowercase());
    api::Clocks {
        now: unix_now(),
        nodes,
    }
}

/// What the reader knows of one listed device.
#[derive(Debug, Default)]
pub struct Peer {
    /// It answered last time it was asked.
    pub answers: bool,
    /// When it is asked next; `None`: now.
    pub next: Option<Instant>,
    /// Unanswered reads in a row while answering.
    pub misses: u32,
}

impl Peer {
    /// Takes a reply read at `now`.
    pub fn answered(&mut self, now: Instant) {
        self.answers = true;
        self.misses = 0;
        self.next = Some(now + READ_EVERY);
    }

    /// Takes a read with no reply at `now`.
    pub fn quiet(&mut self, now: Instant) {
        if self.answers {
            self.misses += 1;
            if self.misses < MISSES {
                self.next = Some(now + READ_EVERY);
                return;
            }
            self.answers = false;
            self.misses = 0;
        }
        self.next = Some(now + RETRY_QUIET);
    }
}

/// Starts the reader thread.
pub fn start(dante: Shared, latest: Latest, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("clocks".into())
        .spawn(move || run(&dante, &latest, &stop))
        .expect("spawn the clock reader");
}

fn run(dante: &Shared, latest: &Latest, stop: &AtomicBool) {
    let mut peers: HashMap<String, Peer> = HashMap::new();
    let mut sock: Option<(Ipv4Addr, std::net::UdpSocket)> = None;
    while !stop.load(Ordering::Relaxed) {
        let (ip, devices) = {
            let st = dante.read().unwrap_or_else(|e| e.into_inner());
            (st.our_ip(), st.with_latency())
        };
        let keys: Vec<String> = devices
            .iter()
            .map(|(d, _)| d.name.to_ascii_lowercase())
            .collect();
        peers.retain(|k, _| keys.contains(k));
        latest
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k, _| keys.contains(k));
        if sock.as_ref().map(|(a, _)| Some(*a)) != Some(ip) {
            sock = ip.and_then(|ip| match client::unicast_socket(ip) {
                Ok(s) => {
                    info!(%ip, "clocks: asking from the Dante address");
                    Some((ip, s))
                }
                Err(e) => {
                    warn!(%ip, error = %e, "clocks: no socket");
                    None
                }
            });
        }
        if let Some((_, s)) = &sock {
            for (d, _) in &devices {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let key = d.name.to_ascii_lowercase();
                let peer = peers.entry(key.clone()).or_default();
                if peer.next.is_some_and(|n| n > Instant::now()) {
                    continue;
                }
                match client::clock_figures(s, *d.arc.ip(), d.device_id, WAIT) {
                    Ok(f) => {
                        let now = Instant::now();
                        if !peer.answers {
                            info!(device = %d.name, source = ?f.source, ready = f.ready,
                                "clocks: device answers its clock figures");
                        }
                        peer.answered(now);
                        let new = node(&d.name, &f);
                        let mut map = latest.write().unwrap_or_else(|e| e.into_inner());
                        // A change of health is worth a line: it is the event.
                        let before = map.get(&key).map(|(n, _)| n.health());
                        if before.as_ref().is_some_and(|b| *b != new.health()) {
                            let (state, why) = new.health();
                            info!(device = %d.name, state, why, "clocks: clock state changed");
                        }
                        map.insert(key, (new, now));
                    }
                    Err(InfoError::Timeout { .. }) => {
                        let was = peer.answers;
                        peer.quiet(Instant::now());
                        if was && !peer.answers {
                            warn!(device = %d.name, misses = MISSES,
                                "clocks: device stopped answering its clock figures");
                        }
                    }
                    Err(e) => {
                        warn!(device = %d.name, error = %e, "clocks: figures not read");
                        peer.quiet(Instant::now());
                    }
                }
            }
        }
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dstack::info::clock_figures::{FollowerFigures, ServiceFigures, poll_of_code};

    fn figures() -> ClockFigures {
        ClockFigures {
            sequence: 7,
            source: TimeSource::Internal,
            ready: true,
            follower: Some(FollowerFigures {
                locked: true,
                settled: true,
                holdover: false,
                waiting: None,
                grandmaster: Some([0, 0x1d, 0xc1, 0xaa, 0xbb, 0xcc]),
                holdovers: 2,
                freq_ppb: -41,
                error_p90_ns: Some(850),
                offset_from_service_ns: None,
            }),
            service: Some(ServiceFigures {
                poll: poll_of_code(2).flatten().expect("poll 2 is no reply"),
                reply_age_ms: Some(1500),
                locked: false,
                settled: false,
                mode: 0,
                holdover: false,
                holdovers: 0,
                freq_ppb: 0,
                grandmaster: None,
            }),
        }
    }

    #[test]
    fn a_read_names_its_source_states_and_grandmaster() {
        let n = node("dsg", &figures());
        assert_eq!(
            (n.device.as_str(), n.source.as_str(), n.ready),
            ("dsg", "follower", true)
        );
        let f = n.follower.as_ref().unwrap();
        assert!(f.locked && f.settled && !f.holdover);
        assert_eq!(f.waiting, None);
        assert_eq!(f.grandmaster.as_deref(), Some("00:1d:c1:aa:bb:cc"));
        assert_eq!(
            (f.holdovers, f.freq_ppb, f.error_p90_ns),
            (2, -41, Some(850))
        );
        let s = n.service.as_ref().unwrap();
        assert_eq!((s.poll.as_str(), s.reply_age_ms), ("no reply", Some(1500)));
        assert_eq!(n.health().0, "ok");
        // A device on the service alone, which does not reply.
        let mut on_service = figures();
        on_service.source = TimeSource::Service;
        on_service.follower = None;
        let n = node("pc", &on_service);
        assert_eq!(n.source, "service");
        assert_eq!(n.health(), ("bad", "dsync service: no reply".to_owned()));
    }

    #[test]
    fn the_list_drops_a_read_gone_stale_and_ages_the_rest() {
        let latest: Latest = Latest::default();
        let now = Instant::now();
        {
            let mut m = latest.write().unwrap();
            m.insert(
                "b".into(),
                (node("B", &figures()), now - Duration::from_secs(3)),
            );
            m.insert(
                "a".into(),
                (
                    node("a", &figures()),
                    now - Duration::from_secs(STALE_S + 5),
                ),
            );
            m.insert("c".into(), (node("c", &figures()), now));
        }
        let c = snapshot(&latest, now);
        let got: Vec<(&str, i64)> = c
            .nodes
            .iter()
            .map(|n| (n.device.as_str(), n.age_s))
            .collect();
        assert_eq!(got, [("B", 3), ("c", 0)]);
    }

    #[test]
    fn a_device_that_stops_answering_is_asked_rarely_after_a_few_misses() {
        let t = Instant::now();
        let mut p = Peer::default();
        // Never answered: asked again only after the long wait.
        p.quiet(t);
        assert_eq!(p.next, Some(t + RETRY_QUIET));
        p.answered(t);
        assert_eq!((p.answers, p.next), (true, Some(t + READ_EVERY)));
        for _ in 1..MISSES {
            p.quiet(t);
            assert!(p.answers);
            assert_eq!(p.next, Some(t + READ_EVERY));
        }
        p.quiet(t);
        assert!(!p.answers);
        assert_eq!(p.next, Some(t + RETRY_QUIET));
    }
}
