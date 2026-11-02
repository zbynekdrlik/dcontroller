//! The latency recorder: packet latency per receiver, as Dante Controller's
//! Latency tab shows it, recorded all the time for every receiver on the
//! network (not only while a page looks), into the history store.
//!
//! Every receiver sends its latency in a heartbeat once a second (dstack
//! `latency`): each RX flow slot's largest lateness that second and its
//! cumulative late packets. One thread keeps one listener on the heartbeat
//! group (`latency::Listener`), matches each heartbeat to a listed device by
//! its address and device id, and turns it into a sample per active flow,
//! stamped when it came. Samples are written every [`FLUSH_EVERY`]. Every
//! [`FLOWS_EVERY`] the thread reads each receiver's flows over ARC (latency,
//! channels, sources) and records a changed description; every
//! [`ROLLUP_EVERY`] it rolls the history up.

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dstack::arc::client::{REPLY_TIMEOUT, Udp};
use dstack::arc::control;
use dstack::ctl::Requests;
use dstack::discover::Device;
use dstack::latency::{Beat, Listener};
use tracing::{info, warn};

use crate::dante::Shared;
use crate::history::{FlowInfo, Sample, Store, unix_now};

/// How often buffered samples are written.
pub const FLUSH_EVERY: Duration = Duration::from_secs(5);
/// How often a receiver's flows (latency, channels, sources) are read again.
pub const FLOWS_EVERY: Duration = Duration::from_secs(60);
/// How often the device list is looked at again (receivers, addresses).
const LOOK_EVERY: Duration = Duration::from_secs(10);
/// How often the history is rolled up and pruned.
const ROLLUP_EVERY: Duration = Duration::from_secs(60);
/// How long the thread waits for a heartbeat before it looks around.
const TICK: Duration = Duration::from_millis(500);

/// A running heartbeat listener: our address, it, and its heartbeats.
type Listening = (Ipv4Addr, Listener, Receiver<(Ipv4Addr, Beat)>);

/// The flows of a device with their RX channel labels and sources.
pub fn describe(flows: &[control::RxFlow], rx: &[control::RxChannel]) -> BTreeMap<u16, FlowInfo> {
    flows
        .iter()
        .filter(|f| !f.channels.is_empty())
        .map(|f| {
            let fed: Vec<&control::RxChannel> = f
                .channels
                .iter()
                .filter_map(|c| rx.iter().find(|r| r.channel == *c))
                .collect();
            let mut sources: Vec<String> = fed.iter().filter_map(|r| r.tx_device.clone()).collect();
            sources.sort_by_key(|s| s.to_ascii_lowercase());
            sources.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
            (
                f.slot,
                FlowInfo {
                    latency_us: f.latency_ns / 1000,
                    rate: f.rate,
                    channels: fed.iter().map(|r| r.label.clone()).collect(),
                    sources,
                },
            )
        })
        .collect()
}

/// The listed device a heartbeat from `from` with `id` belongs to: the
/// device at that address with that device id, or the only device at that
/// address when discovery has no id for it.
pub fn owner(devices: &[Device], from: Ipv4Addr, id: [u8; 8]) -> Option<&Device> {
    let at: Vec<&Device> = devices.iter().filter(|d| *d.arc.ip() == from).collect();
    at.iter()
        .find(|d| d.device_id == Some(id))
        .or_else(|| match at.as_slice() {
            [one] if one.device_id.is_none() => Some(one),
            _ => None,
        })
        .copied()
}

/// One heartbeat's samples at `t` (unix seconds): a peak per active flow
/// (lateness samples at the beat's rate, in µs, as dstack's `Gather`), and
/// the late packets since the last sample stored for that slot (`last`
/// holds each slot's cumulative count; the first heartbeat sets it; a count
/// that went down started again). A heartbeat that stores no sample for a
/// slot (no lateness record, no flow known yet) leaves its count pending.
pub fn beat_samples(
    beat: &Beat,
    active: &BTreeMap<u16, FlowInfo>,
    last: &mut BTreeMap<u16, u32>,
    t: i64,
) -> Vec<Sample> {
    let totals: BTreeMap<u16, u32> = (1u16..).zip(beat.late.iter().flatten().copied()).collect();
    for (&slot, &total) in &totals {
        if active.contains_key(&slot) {
            last.entry(slot).or_insert(total);
        } else {
            // No flow on this slot: its late packets are nobody's.
            last.insert(slot, total);
        }
    }
    let Some((rate, lateness)) = &beat.lateness else {
        return Vec::new();
    };
    (1u16..)
        .zip(lateness)
        .filter(|(slot, _)| active.contains_key(slot))
        .map(|(slot, &samples)| {
            let late = match (totals.get(&slot), last.get(&slot)) {
                (Some(&total), Some(&prev)) => {
                    last.insert(slot, total);
                    if total >= prev { total - prev } else { total }
                }
                _ => 0,
            };
            Sample {
                slot,
                t,
                peak_us: u32::try_from(u64::from(samples) * 1_000_000 / u64::from((*rate).max(1)))
                    .unwrap_or(u32::MAX),
                late,
            }
        })
        .collect()
}

/// Remembers a heartbeat's late counts as the baseline of slots seen for
/// the first time, for a device whose flows are not read yet: its late
/// packets count once a flow is known.
pub fn remember(beat: &Beat, last: &mut BTreeMap<u16, u32>) {
    for (slot, &total) in (1u16..).zip(beat.late.iter().flatten()) {
        last.entry(slot).or_insert(total);
    }
}

/// The second a device's heartbeat is stored at: now, or the second after
/// the device's last one when two came in one second, but never more than
/// two seconds ahead of now (so jitter never shifts a device for good;
/// past that the second is shared and the store merges it).
pub fn stamp(prev: Option<i64>, now: i64) -> i64 {
    match prev {
        Some(p) if p >= now && p < now + 2 => p + 1,
        _ => now,
    }
}

/// Starts the recorder: the heartbeat thread, and the flows thread that
/// keeps each receiver's flows read (so a slow device never holds the
/// heartbeats up).
pub fn start(dante: Shared, store: Arc<Store>, stop: Arc<AtomicBool>) {
    let flows: Flows = Arc::default();
    {
        let (dante, store, stop, flows) = (
            Arc::clone(&dante),
            Arc::clone(&store),
            Arc::clone(&stop),
            Arc::clone(&flows),
        );
        std::thread::Builder::new()
            .name("recorder-flows".into())
            .spawn(move || read_flows_loop(&dante, &store, &stop, &flows))
            .expect("spawn the recorder's flows thread");
    }
    std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || run(&dante, &store, &stop, &flows))
        .expect("spawn the recorder");
}

/// Each receiver's flows as last read, by name (lowercase); shared, so a
/// heartbeat takes them without copying.
type Flows = Arc<Mutex<HashMap<String, Arc<BTreeMap<u16, FlowInfo>>>>>;

/// What the recorder keeps per receiver.
#[derive(Default)]
struct Receiving {
    /// The device's name as discovery lists it (the store matches case aside).
    name: String,
    last_late: BTreeMap<u16, u32>,
    last_t: Option<i64>,
    buffer: Vec<Sample>,
    stored: u64,
}

fn run(dante: &Shared, store: &Store, stop: &AtomicBool, flows: &Flows) {
    let mut listening: Option<Listening> = None;
    let mut devices: Vec<Device> = Vec::new();
    let mut state: HashMap<String, Receiving> = HashMap::new();
    let mut looked: Option<Instant> = None;
    let mut flushed = Instant::now();
    let mut rolled = Instant::now();
    match store.catch_up(unix_now()) {
        Ok(()) => info!("recorder: started, history caught up"),
        Err(e) => warn!(error = %e, "recorder: started, catch-up rollup failed"),
    }
    while !stop.load(Ordering::Relaxed) {
        if looked.is_none_or(|t| t.elapsed() >= LOOK_EVERY) {
            looked = Some(Instant::now());
            let (ip, receivers) = {
                let st = dante.read().unwrap_or_else(|e| e.into_inner());
                (st.our_ip(), st.receivers())
            };
            devices = receivers.into_iter().map(|(_, d)| d).collect();
            let names: Vec<String> = devices
                .iter()
                .map(|d| d.name.to_ascii_lowercase())
                .collect();
            state.retain(|key, r| {
                let keep = names.contains(key);
                if !keep {
                    flush(store, r);
                    info!(device = %r.name, samples = r.stored, "recorder: receiver gone");
                }
                keep
            });
            // The listener follows our address on the Dante interface.
            if listening.as_ref().map(|(at, _, _)| *at) != ip {
                listening = None;
                if let Some(ip) = ip {
                    match Listener::start(ip) {
                        Ok((l, rx)) => {
                            info!("recorder: listening to heartbeats");
                            listening = Some((ip, l, rx));
                        }
                        Err(e) => warn!(error = %e, "recorder: heartbeat listener not started"),
                    }
                }
            }
        }
        let Some((_, _, rx)) = listening.as_ref() else {
            std::thread::sleep(TICK);
            continue;
        };
        match rx.recv_timeout(TICK) {
            Ok((from, beat)) => {
                if let Some(d) = owner(&devices, from, beat.device_id) {
                    let key = d.name.to_ascii_lowercase();
                    let active = flows
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&key)
                        .cloned();
                    let r = state.entry(key).or_default();
                    r.name.clone_from(&d.name);
                    let t = stamp(r.last_t, unix_now());
                    let Some(active) = active else {
                        remember(&beat, &mut r.last_late);
                        continue;
                    };
                    let batch = beat_samples(&beat, &active, &mut r.last_late, t);
                    if !batch.is_empty() {
                        r.last_t = Some(t);
                    }
                    r.buffer.extend(batch);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                warn!("recorder: heartbeat listener ended, starting it again");
                listening = None;
                looked = None;
            }
        }
        if flushed.elapsed() >= FLUSH_EVERY {
            flushed = Instant::now();
            for r in state.values_mut() {
                flush(store, r);
            }
        }
        if rolled.elapsed() >= ROLLUP_EVERY {
            rolled = Instant::now();
            let started = Instant::now();
            match store.rollup(unix_now(), 600) {
                Ok(()) => info!(
                    ms = started.elapsed().as_millis() as u64,
                    "recorder: history rolled up"
                ),
                Err(e) => warn!(error = %e, "recorder: rollup failed"),
            }
        }
    }
    for r in state.values_mut() {
        flush(store, r);
    }
    info!("recorder: stopped");
}

/// Keeps every receiver's flows read, one device at a time, each again
/// after [`FLOWS_EVERY`]; records a changed description.
fn read_flows_loop(dante: &Shared, store: &Store, stop: &AtomicBool, flows: &Flows) {
    let seq = AtomicU16::new(1);
    let requests = Requests::default();
    let mut read_at: HashMap<String, Instant> = HashMap::new();
    while !stop.load(Ordering::Relaxed) {
        let (ip, receivers) = {
            let st = dante.read().unwrap_or_else(|e| e.into_inner());
            (st.our_ip(), st.receivers())
        };
        let names: Vec<String> = receivers
            .iter()
            .map(|(_, d)| d.name.to_ascii_lowercase())
            .collect();
        read_at.retain(|k, _| names.contains(k));
        flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|k, _| names.contains(k));
        if let Some(ip) = ip {
            for (_, d) in &receivers {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let key = d.name.to_ascii_lowercase();
                if read_at.get(&key).is_some_and(|t| t.elapsed() < FLOWS_EVERY) {
                    continue;
                }
                read_at.insert(key.clone(), Instant::now());
                match read_flows(ip, d, &seq, &requests, stop) {
                    Ok(f) => {
                        let now = unix_now();
                        for (slot, info) in &f {
                            if let Err(e) = store.set_flow(&d.name, *slot, info, now) {
                                warn!(device = %d.name, error = %e, "recorder: flow not stored");
                            }
                        }
                        let mut map = flows.lock().unwrap_or_else(|e| e.into_inner());
                        if map.get(&key).is_none_or(|m| m.is_empty()) && !f.is_empty() {
                            info!(device = %d.name, flows = f.len(), "recorder: recording");
                        }
                        map.insert(key, Arc::new(f));
                    }
                    Err(e) => warn!(device = %d.name, error = %e, "recorder: flows not read"),
                }
            }
        }
        let until = Instant::now() + LOOK_EVERY;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Writes a receiver's buffered samples.
fn flush(store: &Store, r: &mut Receiving) {
    if r.buffer.is_empty() {
        return;
    }
    let n = r.buffer.len() as u64;
    match store.add(&r.name, &r.buffer) {
        Ok(()) => r.stored += n,
        Err(e) => warn!(device = %r.name, error = %e, samples = n, "recorder: samples not stored"),
    }
    r.buffer.clear();
}

/// A receiver's flows over ARC, described.
fn read_flows(
    our: Ipv4Addr,
    d: &Device,
    seq: &AtomicU16,
    requests: &Requests,
    stop: &AtomicBool,
) -> Result<BTreeMap<u16, FlowInfo>, String> {
    let mut udp = Udp::open(our, d.arc, seq, REPLY_TIMEOUT, requests, stop)
        .map_err(|e| format!("socket: {e}"))?;
    let f = control::rx_flows(&mut udp).map_err(|e| format!("RX flows: {e}"))?;
    let rx = control::rx_channels(&mut udp).map_err(|e| format!("RX channels: {e}"))?;
    Ok(describe(&f, &rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flows_get_their_channels_and_sources_and_empty_slots_go() {
        let flows = vec![
            control::RxFlow {
                slot: 1,
                rate: 48000,
                latency_ns: 1_000_000,
                channels: vec![1, 2],
            },
            control::RxFlow {
                slot: 2,
                rate: 48000,
                latency_ns: 250_000,
                channels: vec![],
            },
        ];
        let rx = vec![
            control::RxChannel {
                channel: 1,
                label: "In 1".into(),
                tx_channel: Some("A".into()),
                tx_device: Some("desk".into()),
                status: 9,
            },
            control::RxChannel {
                channel: 2,
                label: "In 2".into(),
                tx_channel: Some("B".into()),
                tx_device: Some("DESK".into()),
                status: 9,
            },
        ];
        let d = describe(&flows, &rx);
        assert_eq!(d.len(), 1);
        let f = &d[&1];
        assert_eq!(f.latency_us, 1000);
        assert_eq!(f.channels, ["In 1", "In 2"]);
        assert_eq!(f.sources, ["desk"]);
    }

    fn dev(name: &str, last: u8, id: Option<u8>) -> Device {
        Device {
            name: name.into(),
            arc: std::net::SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, last), 4440),
            dbc: None,
            device_id: id.map(|i| [i; 8]),
            txt: BTreeMap::new(),
        }
    }

    #[test]
    fn late_packets_wait_for_a_heartbeat_that_stores_a_sample() {
        // A heartbeat without a lateness record, or one before the device's
        // flows are read, stores nothing; its late packets must not be lost.
        // (An empty flow map means the device has no flow: see the next test.)
        let active: BTreeMap<u16, FlowInfo> = [(1, FlowInfo::default())].into();
        let mut last = BTreeMap::new();
        let with = |late: u32| Beat {
            device_id: [0; 8],
            lateness: Some((48000, vec![0])),
            late: Some(vec![late]),
        };
        let without = |late: u32| Beat {
            device_id: [0; 8],
            lateness: None,
            late: Some(vec![late]),
        };
        assert_eq!(beat_samples(&with(10), &active, &mut last, 1)[0].late, 0); // the baseline
        assert!(beat_samples(&without(14), &active, &mut last, 2).is_empty());
        // Flows not read yet: the heartbeat is only remembered.
        remember(&with(16), &mut last);
        assert_eq!(beat_samples(&with(17), &active, &mut last, 4)[0].late, 7);
    }

    #[test]
    fn late_packets_of_a_slot_without_a_flow_are_not_credited_later() {
        // A slot with no flow counts late packets nobody records; once a flow
        // starts there, only the packets since count.
        let none: BTreeMap<u16, FlowInfo> = BTreeMap::new();
        let active: BTreeMap<u16, FlowInfo> = [(1, FlowInfo::default())].into();
        let mut last = BTreeMap::new();
        let beat = |late: u32| Beat {
            device_id: [0; 8],
            lateness: Some((48000, vec![0])),
            late: Some(vec![late]),
        };
        assert!(beat_samples(&beat(0), &none, &mut last, 1).is_empty());
        assert!(beat_samples(&beat(50_000), &none, &mut last, 2).is_empty());
        assert_eq!(
            beat_samples(&beat(50_003), &active, &mut last, 3)[0].late,
            3
        );
    }

    #[test]
    fn queued_heartbeats_of_one_device_get_seconds_of_their_own() {
        assert_eq!(stamp(None, 100), 100);
        assert_eq!(stamp(Some(99), 100), 100);
        assert_eq!(stamp(Some(100), 100), 101);
        // Never more than a second or two ahead: one jittered pair of
        // heartbeats must not push the device's later samples into the
        // future for good.
        assert_eq!(stamp(Some(101), 100), 102);
        assert_eq!(stamp(Some(102), 100), 100);
        assert_eq!(stamp(Some(100_000), 100), 100);
        let mut prev = None;
        for now in 0..100 {
            // Two heartbeats in every tenth second.
            for _ in 0..if now % 10 == 0 { 2 } else { 1 } {
                let t = stamp(prev, now);
                assert!(t <= now + 2, "second {now} stamped {t}");
                prev = Some(t);
            }
        }
    }

    #[test]
    fn a_heartbeat_belongs_to_the_device_at_its_address_with_its_id() {
        let devs = vec![
            dev("a", 1, Some(1)),
            dev("b", 1, Some(2)),
            dev("c", 2, None),
            dev("d", 3, None),
            dev("e", 3, None),
        ];
        let ip = |l| Ipv4Addr::new(10, 0, 0, l);
        assert_eq!(
            owner(&devs, ip(1), [2; 8]).map(|d| d.name.as_str()),
            Some("b")
        );
        assert_eq!(owner(&devs, ip(1), [9; 8]), None);
        // The only device at the address and no id from discovery: it.
        assert_eq!(
            owner(&devs, ip(2), [7; 8]).map(|d| d.name.as_str()),
            Some("c")
        );
        // Two at one address, no ids: which is unknown.
        assert_eq!(owner(&devs, ip(3), [7; 8]), None);
        assert_eq!(owner(&devs, ip(4), [7; 8]), None);
    }

    #[test]
    fn a_heartbeat_becomes_a_sample_per_active_flow_with_its_late_packets() {
        let active: BTreeMap<u16, FlowInfo> =
            [(1, FlowInfo::default()), (3, FlowInfo::default())].into();
        let beat = |lat: Vec<u32>, late: Vec<u32>| Beat {
            device_id: [0; 8],
            lateness: Some((48000, lat)),
            late: Some(late),
        };
        let mut last = BTreeMap::new();
        // The first heartbeat only sets the counts; 48 samples at 48 kHz = 1000 µs.
        let s = beat_samples(
            &beat(vec![48, 5, 12], vec![7, 0, 0]),
            &active,
            &mut last,
            100,
        );
        assert_eq!(
            s,
            vec![
                Sample {
                    slot: 1,
                    t: 100,
                    peak_us: 1000,
                    late: 0
                },
                Sample {
                    slot: 3,
                    t: 100,
                    peak_us: 250,
                    late: 0
                },
            ]
        );
        let s = beat_samples(
            &beat(vec![24, 5, 0], vec![10, 0, 2]),
            &active,
            &mut last,
            101,
        );
        assert_eq!(
            s[0],
            Sample {
                slot: 1,
                t: 101,
                peak_us: 500,
                late: 3
            }
        );
        assert_eq!(
            s[1],
            Sample {
                slot: 3,
                t: 101,
                peak_us: 0,
                late: 2
            }
        );
        // A count that went down started again: all of it is new.
        let s = beat_samples(&beat(vec![0, 0, 0], vec![1, 0, 2]), &active, &mut last, 102);
        assert_eq!(s[0].late, 1);
        // No lateness record: nothing to store.
        let none = Beat {
            device_id: [0; 8],
            lateness: None,
            late: Some(vec![1]),
        };
        assert!(beat_samples(&none, &active, &mut last, 103).is_empty());
    }
}
