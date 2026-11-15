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
//!
//! The same meter names two more things dcontroller relays as alarms
//! (dcontroller #93): the hosts that refresh Dante records on the 96 s
//! cycle (`Minute::browsers`, dstack #319), and the dstack peers below the
//! network floor it heard within the hour (`Meter::below_floor`, dstack
//! #321). Each logs a `status` change at its start and at its end, never
//! in between.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dcontroller_api as api;
use dstack::NETWORK_FLOOR;
use dstack::discover::senders::browsers::Browser;
use dstack::discover::senders::{CAP_PER_S, Meter, Minute, Rate};
use dstack::network_floor::Below;
use tracing::{info, warn};

use crate::dante::{ChangeSink, STATUS, Shared};
use crate::history::unix_now;

/// How often the thread looks at the meter and our address.
pub const LOOK_EVERY: Duration = Duration::from_secs(5);
/// The `what` of an mDNS alarm's changes-log entries.
pub const WHAT: &str = "mdns";
/// The `what` of a 96 s browser alarm's entries (dstack #319).
pub const BROWSER: &str = "mdns-browser";
/// The `what` of a network-floor alarm's entries (dstack #321).
pub const FLOOR: &str = "network-floor";
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
        v.floor = NETWORK_FLOOR.to_string();
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
        browsers: m.browsers.iter().map(|b| browser(b, names)).collect(),
    }
}

/// One 96 s browser as the API sends it.
pub fn browser(b: &Browser, names: &dyn Fn(Ipv4Addr) -> Vec<String>) -> api::MdnsBrowser {
    api::MdnsBrowser {
        source: b.source.to_string(),
        devices: names(b.source),
        period_s: period_s(b),
    }
}

/// A browser's cycle, seconds, one decimal.
fn period_s(b: &Browser) -> f64 {
    round(b.period_ns as f64 / 1e9)
}

/// One binary below the floor as the API sends it.
pub fn below(b: &Below) -> api::BelowFloor {
    api::BelowFloor {
        kind: b.kind.word().into(),
        name: b.name.clone(),
        address: b.address.map(|a| a.to_string()),
        version: b.version.to_string(),
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
        entry(WHAT, at, source, names, ok, before, after, message)
    };
    let mut out = Vec::new();
    for r in &m.over_cap {
        let rate = round(m.per_s(r.packets));
        if over.insert(r.source, rate).is_none() {
            out.push(entry(
                r.source,
                false,
                format!("under {CAP_PER_S}/s"),
                format!("{rate:.1}/s"),
                format!(
                    "an mDNS sender is over the cap: {} sends {rate:.1}/s to the group (cap {CAP_PER_S}/s)",
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
            .map(|r| format!("{:.1}/s", m.per_s(r.packets)))
            .unwrap_or_else(|| "quiet".into());
        out.push(entry(
            source,
            true,
            format!("{was:.1}/s"),
            now.clone(),
            format!("the mDNS sender {source} is back under the cap ({now}, cap {CAP_PER_S}/s)"),
        ));
    }
    out
}

/// A `status` entry of the meter's alarm `what` about `source`, named by
/// the devices there (its address when none).
#[allow(clippy::too_many_arguments)]
fn entry(
    what: &str,
    at: u64,
    source: Ipv4Addr,
    names: &dyn Fn(Ipv4Addr) -> Vec<String>,
    ok: bool,
    before: String,
    after: String,
    message: String,
) -> api::ChangeEntry {
    let devices = names(source);
    api::ChangeEntry {
        at,
        user: METER.into(),
        kind: STATUS.into(),
        what: what.into(),
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
}

/// The 96 s browser alarms `m` starts and ends (dstack #319): a host
/// `m.browsers` lists that `browsing` did not (it keeps them, with their
/// cycle), and one in `browsing` that `m` no longer lists.
pub fn browser_alarms(
    browsing: &mut BTreeMap<Ipv4Addr, f64>,
    m: &Minute,
    at: u64,
    names: &dyn Fn(Ipv4Addr) -> Vec<String>,
) -> Vec<api::ChangeEntry> {
    let mut out = Vec::new();
    for b in &m.browsers {
        let p = period_s(b);
        if browsing.insert(b.source, p).is_none() {
            out.push(entry(
                BROWSER,
                at,
                b.source,
                names,
                false,
                "no 96 s cycle".into(),
                format!("every {p:.1} s"),
                format!(
                    "an mDNS host refreshes Dante records every {p:.1} s: a program at {} \
                     browses Dante, and every Dante host answers it at once (dstack #319)",
                    b.source
                ),
            ));
        }
    }
    let ended: Vec<Ipv4Addr> = browsing
        .keys()
        .filter(|s| !m.browsers.iter().any(|b| b.source == **s))
        .copied()
        .collect();
    for source in ended {
        let was = browsing.remove(&source).unwrap_or_default();
        out.push(entry(
            BROWSER,
            at,
            source,
            names,
            true,
            format!("every {was:.1} s"),
            "no 96 s cycle".into(),
            format!("the mDNS host {source} no longer refreshes Dante records on the 96 s cycle"),
        ));
    }
    out
}

/// A binary below the floor's identity: kind, name (case aside), address.
type BelowKey = (String, String, Option<Ipv4Addr>);

fn below_key(b: &Below) -> BelowKey {
    (b.kind.word().into(), b.name.to_ascii_lowercase(), b.address)
}

/// The network-floor alarms `list` (the meter's binaries below the floor
/// now) starts and ends (dstack #321): one `known` did not list (it keeps
/// them, with their name and version), and one in `known` that `list` no longer
/// has (upgraded, or not heard for an hour). A peer is named by its Dante
/// name, at its address.
pub fn floor_alarms(
    known: &mut BTreeMap<BelowKey, (String, String)>,
    list: &[Below],
    at: u64,
) -> Vec<api::ChangeEntry> {
    let floor = NETWORK_FLOOR.to_string();
    let entry = |kind: &str, name: &str, address: Option<Ipv4Addr>, ok, before, after, message| {
        api::ChangeEntry {
            at,
            user: METER.into(),
            kind: STATUS.into(),
            what: FLOOR.into(),
            rx_device: name.into(),
            rx_label: address.map_or_else(|| format!("local {kind}"), |a| a.to_string()),
            name_before: Some(before),
            name_after: Some(after),
            ok,
            message,
            ..api::ChangeEntry::default()
        }
    };
    let at_addr = |a: Option<Ipv4Addr>| a.map(|a| format!(" at {a}")).unwrap_or_default();
    let mut out = Vec::new();
    for b in list {
        let version = b.version.to_string();
        if let std::collections::btree_map::Entry::Vacant(slot) = known.entry(below_key(b)) {
            slot.insert((b.name.clone(), version.clone()));
            out.push(entry(
                b.kind.word(),
                &b.name,
                b.address,
                false,
                format!("floor {floor}"),
                version.clone(),
                format!(
                    "a dstack {} below the network floor: {}{} runs dstack {version} \
                     (floor {floor}): upgrade it (dstack #321)",
                    b.kind.word(),
                    b.name,
                    at_addr(b.address)
                ),
            ));
        }
    }
    let ended: Vec<BelowKey> = known
        .keys()
        .filter(|k| !list.iter().any(|b| below_key(b) == **k))
        .cloned()
        .collect();
    for k in ended {
        let (name, was) = known.remove(&k).unwrap_or_default();
        out.push(entry(
            &k.0,
            &name,
            k.2,
            true,
            was,
            "not listed".into(),
            format!(
                "the dstack {} {name}{} is no longer below the network floor {floor} \
                 (upgraded, or not heard for an hour)",
                k.0,
                at_addr(k.2)
            ),
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
    let mut alarmed = Alarmed::default();
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
        if let Some((_, meter)) = meter.as_ref()
            && let Some(m) = meter.last()
            && seen.as_ref() != Some(&m)
        {
            look(
                dante,
                watch,
                sink,
                &mut alarmed,
                &m,
                &meter.below_floor(),
                our,
            );
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

/// The alarms going on: sources over the cap, 96 s browsers, binaries
/// below the floor.
#[derive(Debug, Default)]
struct Alarmed {
    over: BTreeMap<Ipv4Addr, f64>,
    browsing: BTreeMap<Ipv4Addr, f64>,
    below: BTreeMap<BelowKey, (String, String)>,
}

/// A new minute: its view, and the alarms it and `below` (the meter's
/// binaries below the floor) start or end.
fn look(
    dante: &Shared,
    watch: &Watch,
    sink: &ChangeSink,
    alarmed: &mut Alarmed,
    m: &Minute,
    below_floor: &[Below],
    our: Option<Ipv4Addr>,
) {
    let names = {
        let st = dante.read().unwrap_or_else(|e| e.into_inner());
        let mut all: BTreeMap<Ipv4Addr, Vec<String>> = BTreeMap::new();
        let sources = m.top.iter().chain(&m.over_cap).map(|r| r.source);
        for source in sources.chain(m.browsers.iter().map(|b| b.source)) {
            let mut n = st.names_at(source);
            if Some(source) == our {
                n.push(THIS_SERVER.into());
            }
            all.insert(source, n);
        }
        all
    };
    let names = |ip: Ipv4Addr| names.get(&ip).cloned().unwrap_or_default();
    let at = u64::try_from(unix_now()).unwrap_or_default();
    let view = minute_view(m, at, &names);
    info!(per_s = view.per_s, top = %m.list(&m.top), over_cap = %m.list(&m.over_cap),
        browsers = view.browsers.len(), below_floor = below_floor.len(),
        "mDNS meter: the last minute");
    let mut entries = alarms(&mut alarmed.over, m, at, &names);
    entries.extend(browser_alarms(&mut alarmed.browsing, m, at, &names));
    entries.extend(floor_alarms(&mut alarmed.below, below_floor, at));
    for e in &entries {
        if e.ok {
            info!(what = %e.what, source = %e.rx_label, devices = %e.rx_device,
                before = ?e.name_before, now = ?e.name_after, "mDNS alarm ended: {}", e.message);
        } else {
            warn!(what = %e.what, source = %e.rx_label, devices = %e.rx_device,
                now = ?e.name_after, "mDNS alarm: {}", e.message);
        }
    }
    if !entries.is_empty() {
        sink(&entries);
    }
    let below: Vec<api::BelowFloor> = below_floor.iter().map(below).collect();
    watch.set(|v| {
        v.minute = Some(view);
        v.below_floor = below;
    });
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
            browsers: Vec::new(),
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
            browsers: Vec::new(),
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
    fn the_view_says_the_cap_and_the_floor() {
        let v = Watch::default().view();
        assert_eq!(v.cap_per_s, 50);
        assert_eq!(v.floor, "0.6.58");
    }

    fn browsing(sources: &[(u8, u64)]) -> Minute {
        let mut m = minute(vec![rate(9, 3)]);
        m.browsers = sources
            .iter()
            .map(|&(last, ms)| Browser {
                source: ip(last),
                period_ns: ms * 1_000_000,
                warn: true,
            })
            .collect();
        m
    }

    #[test]
    fn the_view_lists_the_96_s_browsers_with_their_devices() {
        let v = minute_view(&browsing(&[(7, 96_040)]), 1, &names);
        assert_eq!(v.browsers.len(), 1);
        assert_eq!(v.browsers[0].source, "10.0.0.7");
        assert_eq!(v.browsers[0].devices, vec!["mixer".to_owned()]);
        assert_eq!(v.browsers[0].period_s, 96.0);
    }

    #[test]
    fn a_96_s_browser_starts_one_alarm_and_its_end_one_entry() {
        let mut b = BTreeMap::new();
        let m = browsing(&[(7, 96_400)]);
        let start = browser_alarms(&mut b, &m, 10, &names);
        assert_eq!(start.len(), 1);
        let e = &start[0];
        assert!(!e.ok);
        assert_eq!(
            (e.kind.as_str(), e.what.as_str()),
            ("status", "mdns-browser")
        );
        assert_eq!(e.user, METER);
        assert_eq!(
            (e.rx_label.as_str(), e.rx_device.as_str()),
            ("10.0.0.7", "mixer")
        );
        assert_eq!(e.name_before.as_deref(), Some("no 96 s cycle"));
        assert_eq!(e.name_after.as_deref(), Some("every 96.4 s"));
        assert!(
            e.message.contains("every 96.4 s: a program at 10.0.0.7"),
            "{}",
            e.message
        );

        // Listed again, even with another cycle: nothing new.
        assert!(browser_alarms(&mut b, &m, 11, &names).is_empty());
        assert!(browser_alarms(&mut b, &browsing(&[(7, 97_000)]), 12, &names).is_empty());

        // A second host starts its own; the first one's end follows.
        let two = browser_alarms(&mut b, &browsing(&[(7, 96_000), (9, 96_000)]), 13, &names);
        assert_eq!(two.len(), 1);
        assert_eq!(two[0].rx_device, "10.0.0.9");
        let end = browser_alarms(&mut b, &browsing(&[(9, 96_000)]), 20, &names);
        assert_eq!(end.len(), 1);
        assert!(end[0].ok);
        assert_eq!(end[0].rx_label, "10.0.0.7");
        assert_eq!(end[0].name_before.as_deref(), Some("every 96.0 s"));
        assert_eq!(end[0].name_after.as_deref(), Some("no 96 s cycle"));
        assert_eq!(b.len(), 1);
    }

    fn old(name: &str, last: u8, patch: u16) -> Below {
        Below {
            kind: dstack::network_floor::Kind::Peer,
            name: name.into(),
            address: Some(ip(last)),
            version: dstack::network_floor::Version::new(0, 6, patch),
            seen_ns: 0,
        }
    }

    #[test]
    fn a_peer_below_the_floor_starts_one_alarm_and_its_end_one_entry() {
        let mut known = BTreeMap::new();
        let list = vec![old("PC-Studio", 7, 20)];
        let start = floor_alarms(&mut known, &list, 10);
        assert_eq!(start.len(), 1);
        let e = &start[0];
        assert!(!e.ok);
        assert_eq!(
            (e.kind.as_str(), e.what.as_str()),
            ("status", "network-floor")
        );
        assert_eq!(e.user, METER);
        assert_eq!(
            (e.rx_device.as_str(), e.rx_label.as_str()),
            ("PC-Studio", "10.0.0.7")
        );
        assert_eq!(e.name_before.as_deref(), Some("floor 0.6.58"));
        assert_eq!(e.name_after.as_deref(), Some("0.6.20"));
        assert!(
            e.message
                .contains("PC-Studio at 10.0.0.7 runs dstack 0.6.20 (floor 0.6.58)"),
            "{}",
            e.message
        );

        // Listed again (its name's case aside): nothing new.
        assert!(floor_alarms(&mut known, &list, 11).is_empty());
        assert!(floor_alarms(&mut known, &[old("pc-studio", 7, 20)], 12).is_empty());

        let end = floor_alarms(&mut known, &[], 20);
        assert_eq!(end.len(), 1);
        assert!(end[0].ok);
        assert_eq!(end[0].rx_device, "PC-Studio");
        assert_eq!(end[0].name_before.as_deref(), Some("0.6.20"));
        assert_eq!(end[0].name_after.as_deref(), Some("not listed"));
        assert!(
            end[0]
                .message
                .contains("PC-Studio at 10.0.0.7 is no longer below"),
            "{}",
            end[0].message
        );
        assert!(known.is_empty());
        assert!(floor_alarms(&mut known, &[], 21).is_empty());
    }

    #[test]
    fn the_same_name_at_another_address_is_another_alarm_and_a_client_has_no_address() {
        let mut known = BTreeMap::new();
        let mut client = old("dtray", 1, 30);
        client.kind = dstack::network_floor::Kind::Client;
        client.address = None;
        let start = floor_alarms(&mut known, &[old("PC", 7, 20), old("PC", 8, 20), client], 1);
        assert_eq!(start.len(), 3);
        assert_eq!(start[2].rx_label, "local client");
        assert!(
            start[2]
                .message
                .starts_with("a dstack client below the network floor: dtray runs"),
            "{}",
            start[2].message
        );
        let b = below(&old("PC", 7, 20));
        assert_eq!(
            (b.kind.as_str(), b.address.as_deref(), b.version.as_str()),
            ("peer", Some("10.0.0.7"), "0.6.20")
        );
    }
}
