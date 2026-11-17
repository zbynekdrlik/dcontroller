//! The bridge reader (dcontroller #108): a dasio bridge device's own figures
//! (its ASIO targets, converter delays, underruns and their causes, driver
//! callback lateness), asked once a second with dstack's info request
//! (dstack #391, `dstack::info::client::bridge_figures`) and stored every
//! [`STORE_EVERY`] seconds in the history store for `GET /api/bridge`.
//!
//! Only a dstack device with a bridge answers. Every listed device is asked
//! once; one that does not answer is asked again only after [`RETRY_QUIET`]
//! (a dasio started later, a device that came back), so the reader sends
//! one request a second per bridge and a few an hour per other device.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dcontroller_api as api;
use dstack::info::client::{self, InfoError};
use dstack::info::{BridgeFigures, UnderrunCause};
use tracing::{info, warn};

use crate::dante::Shared;
use crate::history::{Store, unix_now};

/// How often a bridge device is asked.
pub const READ_EVERY: Duration = Duration::from_secs(1);
/// How often a read is stored, seconds (the underruns tick every 10 s).
pub const STORE_EVERY: i64 = 10;
/// How long one request waits for its reply.
pub const WAIT: Duration = Duration::from_millis(300);
/// How long a device that did not answer waits before it is asked again.
pub const RETRY_QUIET: Duration = Duration::from_secs(300);
/// How long a bridge's reads wait for its latency before they are stored
/// without it.
pub const LATENCY_WAIT: Duration = Duration::from_secs(60);
/// Unanswered reads in a row after which a bridge is taken as gone quiet.
pub const MISSES: u32 = 10;

/// A cause as the API names it.
pub fn cause_name(c: UnderrunCause) -> &'static str {
    match c {
        UnderrunCause::None => "none",
        UnderrunCause::Bridge => "bridge",
        UnderrunCause::Driver => "driver",
        UnderrunCause::Unknown => "unknown",
    }
}

/// One read as stored: the figures, the device's latency then, at `t`.
pub fn point(f: &BridgeFigures, latency_us: u32, t: i64) -> api::BridgePoint {
    api::BridgePoint {
        t,
        latency_us,
        to_asio_target_us: f.to_asio_target_us,
        from_asio_target_us: f.from_asio_target_us,
        to_asio_converter_delay_us: f.to_asio_converter_delay_us,
        from_asio_converter_delay_us: f.from_asio_converter_delay_us,
        to_asio_underruns: f.to_asio_underruns,
        from_asio_underruns: f.from_asio_underruns,
        to_asio_cause: cause_name(f.to_asio_cause).to_owned(),
        from_asio_cause: cause_name(f.from_asio_cause).to_owned(),
        late_callbacks: f.late_callbacks,
        late_bins: f.late_bins.to_vec(),
        run_start_unix_ms: f.run_start_unix_ms,
    }
}

/// What the reader knows of one listed device.
#[derive(Debug, Default)]
pub struct Peer {
    /// It answered last time it was asked.
    pub bridge: bool,
    /// When it is asked next; `None`: now.
    pub next: Option<Instant>,
    /// Unanswered reads in a row while a bridge.
    pub misses: u32,
    /// The last reply's sequence and run (a late reply to an earlier ask
    /// repeats one; a restart starts a new run).
    pub last: Option<(u64, u32)>,
    /// The second the last read was stored.
    pub stored_t: i64,
    /// When it first answered (this listing).
    pub first: Option<Instant>,
}

impl Peer {
    /// Takes a reply read at `now`: true when it is new (not one seen).
    pub fn answered(&mut self, f: &BridgeFigures, now: Instant) -> bool {
        self.misses = 0;
        self.next = Some(now + READ_EVERY);
        let key = (f.run_start_unix_ms, f.sequence);
        let new = self.last != Some(key);
        self.last = Some(key);
        self.bridge = true;
        self.first.get_or_insert(now);
        new
    }

    /// Takes a read with no reply at `now`.
    pub fn quiet(&mut self, now: Instant) {
        if self.bridge {
            self.misses += 1;
            if self.misses < MISSES {
                self.next = Some(now + READ_EVERY);
                return;
            }
            self.bridge = false;
            self.misses = 0;
        }
        self.next = Some(now + RETRY_QUIET);
    }

    /// Whether a read at second `t` is to be stored: every
    /// [`STORE_EVERY`], once the device's latency is known (its settings
    /// are read a moment after it is listed), or after [`LATENCY_WAIT`]
    /// without it (a device whose settings never read).
    pub fn store_due(&mut self, t: i64, latency_known: bool, now: Instant) -> bool {
        let waited = self
            .first
            .is_some_and(|f| now.duration_since(f) >= LATENCY_WAIT);
        if (latency_known || waited) && t - self.stored_t >= STORE_EVERY {
            self.stored_t = t;
            true
        } else {
            false
        }
    }
}

/// Starts the reader thread.
pub fn start(dante: Shared, store: Arc<Store>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("bridge".into())
        .spawn(move || run(&dante, &store, &stop))
        .expect("spawn the bridge reader");
}

fn run(dante: &Shared, store: &Store, stop: &AtomicBool) {
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
        if sock.as_ref().map(|(a, _)| Some(*a)) != Some(ip) {
            sock = ip.and_then(|ip| match client::unicast_socket(ip) {
                Ok(s) => {
                    info!(%ip, "bridge: asking from the Dante address");
                    Some((ip, s))
                }
                Err(e) => {
                    warn!(%ip, error = %e, "bridge: no socket");
                    None
                }
            });
        }
        if let Some((_, s)) = &sock {
            for (d, lat) in &devices {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let peer = peers.entry(d.name.to_ascii_lowercase()).or_default();
                let now = Instant::now();
                if peer.next.is_some_and(|n| n > now) {
                    continue;
                }
                match client::bridge_figures(s, *d.arc.ip(), d.device_id, WAIT) {
                    Ok(f) => {
                        let was = peer.bridge;
                        if !peer.answered(&f, Instant::now()) {
                            continue;
                        }
                        if !was {
                            info!(device = %d.name, to_target_us = f.to_asio_target_us,
                                from_target_us = f.from_asio_target_us, "bridge: device answers its figures");
                        }
                        let t = unix_now();
                        if peer.store_due(t, lat.is_some(), Instant::now())
                            && let Err(e) =
                                store.add_bridge(&d.name, &point(&f, lat.unwrap_or(0), t))
                        {
                            warn!(device = %d.name, error = %e, "bridge: read not stored");
                        }
                    }
                    Err(InfoError::Timeout { .. }) => {
                        let was = peer.bridge;
                        peer.quiet(Instant::now());
                        if was && !peer.bridge {
                            warn!(device = %d.name, misses = MISSES, "bridge: device stopped answering its figures");
                        }
                    }
                    Err(e) => {
                        warn!(device = %d.name, error = %e, "bridge: figures not read");
                        peer.quiet(Instant::now());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn figures(seq: u32, run: u64) -> BridgeFigures {
        BridgeFigures {
            sequence: seq,
            run_start_unix_ms: run,
            to_asio_target_us: 1000,
            from_asio_target_us: 1667,
            to_asio_converter_delay_us: 167,
            from_asio_converter_delay_us: 167,
            to_asio_underruns: 4,
            from_asio_underruns: 1,
            to_asio_cause: UnderrunCause::Driver,
            from_asio_cause: UnderrunCause::Bridge,
            late_callbacks: 2,
            late_bins: [8, 7, 6, 5, 4, 3, 2, 1],
        }
    }

    #[test]
    fn a_read_becomes_a_point_with_the_device_latency_and_named_causes() {
        let p = point(&figures(3, 99), 1000, 50);
        assert_eq!(p.t, 50);
        assert_eq!(p.latency_us, 1000);
        assert_eq!(p.to_asio_cause, "driver");
        assert_eq!(p.from_asio_cause, "bridge");
        assert_eq!(p.late_bins, vec![8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(p.run_start_unix_ms, 99);
        assert_eq!(p.dante_to_asio_us(), 2167);
        assert_eq!(p.asio_to_dante_us(), 1834);
        assert_eq!(cause_name(UnderrunCause::None), "none");
        assert_eq!(cause_name(UnderrunCause::Unknown), "unknown");
    }

    #[test]
    fn a_repeated_reply_is_dropped_and_a_restart_is_new() {
        let t0 = Instant::now();
        let mut p = Peer::default();
        assert!(p.answered(&figures(1, 10), t0));
        assert!(p.bridge);
        assert_eq!(p.next, Some(t0 + READ_EVERY));
        assert!(!p.answered(&figures(1, 10), t0));
        assert!(p.answered(&figures(2, 10), t0));
        // A restart: a new run, the sequence from 1 again.
        assert!(p.answered(&figures(1, 20), t0));
    }

    #[test]
    fn a_quiet_device_is_asked_rarely_and_a_bridge_after_misses_only() {
        let t0 = Instant::now();
        let mut p = Peer::default();
        p.quiet(t0);
        assert!(!p.bridge);
        assert_eq!(p.next, Some(t0 + RETRY_QUIET));
        p.answered(&figures(1, 1), t0);
        for _ in 0..MISSES - 1 {
            p.quiet(t0);
            assert!(p.bridge);
            assert_eq!(p.next, Some(t0 + READ_EVERY));
        }
        p.quiet(t0);
        assert!(!p.bridge);
        assert_eq!(p.next, Some(t0 + RETRY_QUIET));
    }

    #[test]
    fn a_read_is_stored_every_ten_seconds_once_the_latency_is_known() {
        let t0 = Instant::now();
        let mut p = Peer::default();
        p.answered(&figures(1, 1), t0);
        // No latency yet: not stored (a 0 would be a wrong total).
        assert!(!p.store_due(100, false, t0));
        assert!(p.store_due(100, true, t0));
        assert!(!p.store_due(105, true, t0));
        assert!(!p.store_due(109, true, t0));
        assert!(p.store_due(110, true, t0));
        // Never read: stored without it after the wait.
        assert!(!p.store_due(130, false, t0 + LATENCY_WAIT - Duration::from_secs(1)));
        assert!(p.store_due(130, false, t0 + LATENCY_WAIT));
    }
}
