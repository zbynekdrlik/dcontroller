//! The devices this server has listed (#122): each one's last record, kept
//! after it leaves the network, so the Map keeps the box of a device that
//! was switched off, with its place and its last subscriptions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dcontroller_api as api;
use tracing::{info, warn};

use crate::dante;
use crate::history::{Store, unix_now};

/// How often the listed devices are written down.
const EVERY: Duration = Duration::from_secs(30);

/// Writes down every listed device every [`EVERY`], until `stop`.
pub fn start(dante: dante::Shared, store: Arc<Store>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("known-devices".into())
        .spawn(move || {
            info!(
                every_s = EVERY.as_secs(),
                "known devices: remembering what is listed"
            );
            while !stop.load(Ordering::Relaxed) {
                let listed = dante::snapshot(&dante).devices;
                if let Err(e) = store.remember(unix_now(), &listed) {
                    warn!(error = %e, devices = listed.len(), "known devices: not written");
                }
                let mut slept = Duration::ZERO;
                while slept < EVERY && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    slept += Duration::from_millis(500);
                }
            }
        })
        .expect("spawn the known-devices thread");
}

/// The devices remembered that `listed` does not hold (case aside), the
/// most recently seen first.
pub fn absent(store: &Store, listed: &[api::Device], now: i64) -> Vec<api::Absent> {
    let known = match store.known() {
        Ok(k) => k,
        Err(e) => {
            warn!(error = %e, "known devices: not read");
            return Vec::new();
        }
    };
    let mut out: Vec<api::Absent> = known
        .into_iter()
        .filter(|(d, _)| !listed.iter().any(|l| l.name.eq_ignore_ascii_case(&d.name)))
        .map(|(device, last_seen)| api::Absent {
            device,
            last_seen: u64::try_from(last_seen).unwrap_or(0),
            ago_s: u64::try_from(now - last_seen).unwrap_or(0),
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then(a.device.name.cmp(&b.device.name))
    });
    out
}
