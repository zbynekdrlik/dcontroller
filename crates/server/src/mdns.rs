//! Who sends how much mDNS on the Dante leg (dstack #289, dcontroller #86).
//! A flood of mDNS (one program at about 40 queries a second, dstack #285)
//! costs every host on the Dante VLAN packets; this names the sender.
//!
//! dstack's devices put the same numbers into their status, but that status
//! is each PC's local control socket. mDNS goes to the 224.0.0.251 group,
//! which every host on the VLAN receives, so the server runs dstack's own
//! meter (`discover::senders::Meter`, passive: it sends nothing) on its
//! Dante address and reads its last closed minute. One thread looks every
//! [`LOOK_EVERY`]: it opens the meter once discovery has our address, keeps
//! the minute for `GET /api/mdns`, and logs a `status` change when a source
//! goes over [`CAP_PER_S`] and when it is back under.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dcontroller_api as api;
use dstack::discover::senders::{CAP_PER_S, Meter, Minute, Rate};
use tracing::{info, warn};

use crate::dante::{ChangeSink, STATUS, Shared};
use crate::history::unix_now;

/// How often the thread looks at the meter and our address.
pub const LOOK_EVERY: Duration = Duration::from_secs(5);
/// The `what` of an mDNS alarm's changes-log entries.
pub const WHAT: &str = "mdns";
/// Who an mDNS alarm is logged as.
pub const METER: &str = "mDNS meter";
/// The name our own address is listed under.
pub const THIS_SERVER: &str = "this server";

/// The last minute, as `GET /api/mdns` sends it.
#[derive(Debug, Default)]
pub struct Watch {
    view: Mutex<api::Mdns>,
}

impl Watch {
    pub fn view(&self) -> api::Mdns {
        let mut v = self.view.lock().unwrap_or_else(|e| e.into_inner()).clone();
        v.cap_per_s = CAP_PER_S;
        v
    }

    fn set(&self, f: impl FnOnce(&mut api::Mdns)) {
        f(&mut self.view.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

/// One source of `m` with the devices `names` lists at its address.
pub fn sender(m: &Minute, r: &Rate, names: &dyn Fn(Ipv4Addr) -> Vec<String>) -> api::MdnsSender {
    api::MdnsSender {
        source: r.source.to_string(),
        devices: names(r.source),
        per_s: round(m.per_s(r.packets)),
    }
}

/// `m` as the API sends it, read at `at`.
pub fn minute_view(
    m: &Minute,
    at: u64,
    names: &dyn Fn(Ipv4Addr) -> Vec<String>,
) -> api::MdnsMinute {
    api::MdnsMinute {
        at,
        per_s: round(m.per_s(m.total)),
        top: m.top.iter().map(|r| sender(m, r, names)).collect(),
        over_cap: m.over_cap.iter().map(|r| sender(m, r, names)).collect(),
    }
}

/// One decimal, as dstack's log lines show a rate.
fn round(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// The alarms `m` starts and ends, as changes-log entries at `at`: a source
/// over the cap that was not (`over` keeps them, with their last rate), and
/// one in `over` that `m` no longer lists over the cap. The same minute
/// read again logs nothing.
pub fn alarms(
    over: &mut BTreeMap<Ipv4Addr, f64>,
    m: &Minute,
    at: u64,
    names: &dyn Fn(Ipv4Addr) -> Vec<String>,
) -> Vec<api::ChangeEntry> {
    let entry = |source: Ipv4Addr, ok: bool, before: String, after: String, message: String| {
        let devices = names(source);
        api::ChangeEntry {
            at,
            user: METER.into(),
            kind: STATUS.into(),
            what: WHAT.into(),
            rx_device: if devices.is_empty() {
                source.to_string()
            } else {
                devices.join(", ")
            },
            rx_label: source.to_string(),
            name_before: Some(before),
            name_after: Some(after),
            ok,
            message,
            ..api::ChangeEntry::default()
        }
    };
    let mut out = Vec::new();
    for r in &m.over_cap {
        let rate = round(m.per_s(r.packets));
        if over.insert(r.source, rate).is_none() {
            out.push(entry(
                r.source,
                false,
                format!("under {CAP_PER_S}/s"),
                format!("{rate}/s"),
                format!(
                    "an mDNS sender is over the cap: {} sends {rate}/s to the group (cap {CAP_PER_S}/s)",
                    r.source
                ),
            ));
        }
    }
    let ended: Vec<Ipv4Addr> = over
        .keys()
        .filter(|s| !m.over_cap.iter().any(|r| r.source == **s))
        .copied()
        .collect();
    for source in ended {
        let was = over.remove(&source).unwrap_or_default();
        let now = m
            .top
            .iter()
            .find(|r| r.source == source)
            .map(|r| format!("{}/s", round(m.per_s(r.packets))))
            .unwrap_or_else(|| "quiet".into());
        out.push(entry(
            source,
            true,
            format!("{was}/s"),
            now.clone(),
            format!("the mDNS sender {source} is back under the cap ({now}, cap {CAP_PER_S}/s)"),
        ));
    }
    out
}

/// Starts the meter thread; it ends when `stop` is set.
pub fn start(dante: Shared, watch: Arc<Watch>, sink: ChangeSink, stop: Arc<AtomicBool>) {
    let ours = Arc::clone(&watch);
    let spawned = std::thread::Builder::new()
        .name("mdns-meter".into())
        .spawn(move || run(&dante, &ours, &sink, &stop));
    if let Err(e) = spawned {
        warn!(error = %e, "mDNS meter: thread not started; /api/mdns stays empty");
        watch_error(&watch, format!("thread not started: {e}"));
    }
}

fn watch_error(watch: &Watch, error: String) {
    watch.set(|v| {
        v.running = false;
        v.error = Some(error);
    });
}

fn run(dante: &Shared, watch: &Watch, sink: &ChangeSink, stop: &AtomicBool) {
    let mut meter: Option<(Ipv4Addr, Arc<Meter>)> = None;
    let mut over = BTreeMap::new();
    let mut seen: Option<Minute> = None;
    let mut failed: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        let our = dante.read().unwrap_or_else(|e| e.into_inner()).our_ip();
        match our {
            None => {
                if meter.take().is_some() {
                    info!("mDNS meter: stopped (no Dante address)");
                }
                watch_error(watch, "no Dante address (discovery does not run)".into());
            }
            Some(ip) if meter.as_ref().is_none_or(|(at, _)| *at != ip) => match Meter::shared(ip) {
                Ok(m) => {
                    info!(address = %ip, "mDNS meter: counting mDNS senders on the Dante leg (passive)");
                    meter = Some((ip, m));
                    failed = None;
                    watch.set(|v| {
                        v.running = true;
                        v.error = None;
                    });
                }
                Err(e) => {
                    let e = format!("the meter does not open on {ip}: {e}");
                    if failed.as_ref() != Some(&e) {
                        warn!(error = %e, "mDNS meter: not started; tried again in 5 s");
                        failed = Some(e.clone());
                    }
                    watch_error(watch, e);
                }
            },
            Some(_) => {}
        }
        if let Some(m) = meter.as_ref().and_then(|(_, m)| m.last())
            && seen.as_ref() != Some(&m)
        {
            look(dante, watch, sink, &mut over, &m, our);
            seen = Some(m);
        }
        for _ in 0..10 {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(LOOK_EVERY / 10);
        }
    }
}

/// A new minute: its view, and the alarms it starts or ends.
fn look(
    dante: &Shared,
    watch: &Watch,
    sink: &ChangeSink,
    over: &mut BTreeMap<Ipv4Addr, f64>,
    m: &Minute,
    our: Option<Ipv4Addr>,
) {
    let names = {
        let st = dante.read().unwrap_or_else(|e| e.into_inner());
        let mut all: BTreeMap<Ipv4Addr, Vec<String>> = BTreeMap::new();
        for r in m.top.iter().chain(&m.over_cap) {
            let mut n = st.names_at(r.source);
            if Some(r.source) == our {
                n.push(THIS_SERVER.into());
            }
            all.insert(r.source, n);
        }
        all
    };
    let names = |ip: Ipv4Addr| names.get(&ip).cloned().unwrap_or_default();
    let at = u64::try_from(unix_now()).unwrap_or_default();
    let view = minute_view(m, at, &names);
    info!(per_s = view.per_s, top = %m.list(&m.top), over_cap = %m.list(&m.over_cap),
        "mDNS meter: the last minute");
    let entries = alarms(over, m, at, &names);
    for e in &entries {
        if e.ok {
            info!(source = %e.rx_label, devices = %e.rx_device, now = ?e.name_after,
                "mDNS alarm ended: the sender is back under the cap");
        } else {
            warn!(source = %e.rx_label, devices = %e.rx_device, per_s = ?e.name_after,
                cap_per_s = CAP_PER_S, "mDNS alarm: a sender is over the cap");
        }
    }
    if !entries.is_empty() {
        sink(&entries);
    }
    watch.set(|v| v.minute = Some(view));
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_NS: u64 = 60_000_000_000;

    fn ip(last: u8) -> Ipv4Addr {
        Ipv4Addr::new(10, 0, 0, last)
    }

    fn rate(last: u8, per_s: u64) -> Rate {
        Rate {
            source: ip(last),
            packets: per_s * 60,
        }
    }

    fn minute(top: Vec<Rate>) -> Minute {
        let over_cap = top
            .iter()
            .filter(|r| r.packets > CAP_PER_S * 60)
            .copied()
            .collect();
        Minute {
            ns: MIN_NS,
            total: top.iter().map(|r| r.packets).sum(),
            top,
            over_cap,
        }
    }

    fn names(ip: Ipv4Addr) -> Vec<String> {
        if ip.octets()[3] == 7 {
            vec!["mixer".into()]
        } else {
            Vec::new()
        }
    }

    #[test]
    fn the_view_gives_each_source_its_rate_and_devices() {
        let m = minute(vec![rate(7, 80), rate(9, 3)]);
        let v = minute_view(&m, 42, &names);
        assert_eq!(v.at, 42);
        assert_eq!(v.per_s, 83.0);
        assert_eq!(v.top.len(), 2);
        assert_eq!(v.top[0].source, "10.0.0.7");
        assert_eq!(v.top[0].devices, vec!["mixer".to_owned()]);
        assert_eq!(v.top[0].per_s, 80.0);
        assert_eq!(v.top[1].devices, Vec::<String>::new());
        assert_eq!(v.over_cap.len(), 1);
        assert_eq!(v.over_cap[0].source, "10.0.0.7");
    }

    #[test]
    fn a_rate_shows_one_decimal() {
        let m = Minute {
            ns: MIN_NS,
            total: 100,
            top: vec![Rate {
                source: ip(1),
                packets: 100,
            }],
            over_cap: Vec::new(),
        };
        assert_eq!(minute_view(&m, 0, &names).per_s, 1.7);
    }

    #[test]
    fn a_source_over_the_cap_starts_one_alarm_and_its_end_one_entry() {
        let mut over = BTreeMap::new();
        let flood = minute(vec![rate(7, 80), rate(9, 3)]);
        let start = alarms(&mut over, &flood, 10, &names);
        assert_eq!(start.len(), 1);
        let e = &start[0];
        assert!(!e.ok);
        assert_eq!(e.kind, "status");
        assert_eq!(e.what, "mdns");
        assert_eq!(e.user, METER);
        assert_eq!(e.rx_label, "10.0.0.7");
        assert_eq!(e.rx_device, "mixer");
        assert_eq!(e.name_before.as_deref(), Some("under 50/s"));
        assert_eq!(e.name_after.as_deref(), Some("80.0/s"));
        assert!(e.message.contains("10.0.0.7 sends 80.0/s"), "{}", e.message);
        assert_eq!(e.at, 10);

        // The same minute again, or a further minute over the cap: nothing new.
        assert!(alarms(&mut over, &flood, 11, &names).is_empty());
        assert!(alarms(&mut over, &minute(vec![rate(7, 70)]), 12, &names).is_empty());

        let calm = minute(vec![rate(7, 4)]);
        let end = alarms(&mut over, &calm, 20, &names);
        assert_eq!(end.len(), 1);
        assert!(end[0].ok);
        assert_eq!(end[0].name_before.as_deref(), Some("70.0/s"));
        assert_eq!(end[0].name_after.as_deref(), Some("4.0/s"));
        assert!(over.is_empty());
        assert!(alarms(&mut over, &calm, 21, &names).is_empty());
    }

    #[test]
    fn a_source_without_a_device_is_named_by_its_address_and_a_gone_one_is_quiet() {
        let mut over = BTreeMap::new();
        let start = alarms(&mut over, &minute(vec![rate(9, 60)]), 1, &names);
        assert_eq!(start[0].rx_device, "10.0.0.9");
        let end = alarms(&mut over, &minute(vec![rate(7, 2)]), 2, &names);
        assert_eq!(end[0].rx_label, "10.0.0.9");
        assert_eq!(end[0].name_after.as_deref(), Some("quiet"));
    }

    #[test]
    fn exactly_the_cap_is_no_alarm() {
        let mut over = BTreeMap::new();
        assert!(alarms(&mut over, &minute(vec![rate(7, CAP_PER_S)]), 1, &names).is_empty());
    }

    #[test]
    fn the_view_says_the_cap() {
        assert_eq!(Watch::default().view().cap_per_s, 50);
    }
}
