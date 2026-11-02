//! The latency recorder: packet latency per receiver, as Dante Controller's
//! Latency tab shows it, recorded all the time for every receiver on the
//! network (not only while a page looks), into the history store.
//!
//! A receiver says its latency in its heartbeat once a second (dstack
//! `latency`): each RX flow slot's largest lateness that second and its late
//! packets. One thread per receiving device listens in windows of
//! [`LISTEN`], takes the device's heartbeats by its device id, and stores a
//! sample per second and active flow. Every [`FLOWS_EVERY`] it reads the
//! device's flows over ARC (latency, channels, sources) and records any
//! change. A supervisor starts and ends the threads as receivers come and
//! go, and rolls the history up every minute.

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dstack::arc::client::{REPLY_TIMEOUT, Udp};
use dstack::arc::control;
use dstack::ctl::Requests;
use dstack::discover::Device;
use dstack::latency;
use tracing::{info, warn};

use crate::dante::Shared;
use crate::history::{FlowInfo, Sample, Store};

/// One listen: this many heartbeats' worth.
pub const LISTEN: Duration = Duration::from_secs(5);
/// How often a receiver's flows (latency, channels, sources) are read again.
pub const FLOWS_EVERY: Duration = Duration::from_secs(60);
/// How often the supervisor looks for receivers that came or went.
const LOOK_EVERY: Duration = Duration::from_secs(10);
/// How often the history is rolled up and pruned.
const ROLLUP_EVERY: Duration = Duration::from_secs(60);

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

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

/// What one listen at a device's address heard.
#[derive(Debug, PartialEq, Eq)]
pub enum Pick {
    /// Nothing.
    None,
    /// One device: it.
    One(latency::Latency),
    /// This many devices at the address: which one is ours is not known.
    Shared(usize),
}

/// The device's own heartbeats among those heard at its address: by its
/// device id (its `_netaudio-cmc` record) when discovery has it, else the
/// single device heard; several devices and no id: which is ours is
/// unknown (two of our own devices can share an address on their own ports).
pub fn pick(mut heard: Vec<latency::Latency>, id: Option<[u8; 8]>) -> Pick {
    if let Some(id) = id {
        return match heard.iter().position(|l| l.device_id == id) {
            Some(i) => Pick::One(heard.swap_remove(i)),
            None if heard.is_empty() => Pick::None,
            // Heartbeats from the address, none with its id: not this device's.
            None => Pick::Shared(heard.len()),
        };
    }
    match heard.len() {
        0 => Pick::None,
        1 => Pick::One(heard.remove(0)),
        n => Pick::Shared(n),
    }
}

/// The samples of one listen of a device that ended at `t_end` (unix
/// seconds): one per heartbeat and active flow, the last heartbeat at
/// `t_end`, the listen's late packets on its last second. Slots without a
/// flow (`active`) are left out.
pub fn samples(l: &latency::Latency, active: &BTreeMap<u16, FlowInfo>, t_end: i64) -> Vec<Sample> {
    let mut out = Vec::new();
    for s in l.slots.iter().filter(|s| active.contains_key(&s.slot)) {
        let n = s.peaks_us.len();
        for (i, &peak_us) in s.peaks_us.iter().enumerate() {
            let last = i + 1 == n;
            out.push(Sample {
                slot: s.slot,
                t: t_end - i64::try_from(n - 1 - i).unwrap_or(0),
                peak_us,
                late: if last {
                    u32::try_from(s.late).unwrap_or(u32::MAX)
                } else {
                    0
                },
            });
        }
    }
    out
}

/// Starts the recorder: a supervisor thread that keeps one recording
/// thread per receiving device and rolls the history up.
pub fn start(dante: Shared, store: Arc<Store>, stop: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("recorder".into())
        .spawn(move || supervise(&dante, &store, &stop))
        .expect("spawn the recorder");
}

fn supervise(dante: &Shared, store: &Arc<Store>, stop: &Arc<AtomicBool>) {
    let mut running: HashMap<String, Arc<AtomicBool>> = HashMap::new();
    let mut rolled = Instant::now();
    info!("recorder: started");
    while !stop.load(Ordering::Relaxed) {
        let receivers = dante.read().unwrap_or_else(|e| e.into_inner()).receivers();
        let names: Vec<String> = receivers
            .iter()
            .map(|(_, d)| d.name.to_ascii_lowercase())
            .collect();
        running.retain(|name, quit| {
            let keep = names.contains(name) && !quit.load(Ordering::Relaxed);
            if !keep {
                quit.store(true, Ordering::Relaxed);
                info!(device = %name, "recorder: receiver gone, recording ended");
            }
            keep
        });
        for (our, device) in receivers {
            let key = device.name.to_ascii_lowercase();
            if running.contains_key(&key) {
                continue;
            }
            let quit = Arc::new(AtomicBool::new(false));
            running.insert(key, Arc::clone(&quit));
            let (store, stop, dante) = (Arc::clone(store), Arc::clone(stop), Arc::clone(dante));
            info!(device = %device.name, "recorder: recording");
            let spawned = std::thread::Builder::new()
                .name("record".into())
                .spawn(move || record(our, &device, &dante, &store, &stop, &quit));
            if let Err(e) = spawned {
                warn!(error = %e, "recorder: thread not started");
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
        let until = Instant::now() + LOOK_EVERY;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    for quit in running.values() {
        quit.store(true, Ordering::Relaxed);
    }
    info!("recorder: stopped");
}

/// Records one receiver until it goes (its records change: the supervisor
/// starts it again) or the server stops.
fn record(
    our: Ipv4Addr,
    device: &Device,
    dante: &Shared,
    store: &Store,
    stop: &AtomicBool,
    quit: &AtomicBool,
) {
    let seq = AtomicU16::new(1);
    let requests = Requests::default();
    let mut flows: BTreeMap<u16, FlowInfo> = BTreeMap::new();
    let mut flows_read: Option<Instant> = None;
    let mut stored: u64 = 0;
    let gone = || {
        stop.load(Ordering::Relaxed)
            || quit.load(Ordering::Relaxed)
            || dante
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .target(&device.name)
                .is_none_or(|(_, d)| d != *device)
    };
    while !gone() {
        if flows_read.is_none_or(|t| t.elapsed() >= FLOWS_EVERY) {
            flows_read = Some(Instant::now());
            let read = Udp::open(our, device.arc, &seq, REPLY_TIMEOUT, &requests, stop)
                .map_err(|e| format!("socket: {e}"))
                .and_then(|mut udp| {
                    let f = control::rx_flows(&mut udp).map_err(|e| format!("RX flows: {e}"))?;
                    let rx =
                        control::rx_channels(&mut udp).map_err(|e| format!("RX channels: {e}"))?;
                    Ok(describe(&f, &rx))
                });
            match read {
                Ok(f) => {
                    let now = unix_now();
                    for (slot, info) in &f {
                        if let Err(e) = store.set_flow(&device.name, *slot, info, now) {
                            warn!(device = %device.name, error = %e, "recorder: flow not stored");
                        }
                    }
                    flows = f;
                }
                Err(e) => warn!(device = %device.name, error = %e, "recorder: flows not read"),
            }
        }
        match latency::listen(our, *device.arc.ip(), LISTEN)
            .map(|heard| pick(heard, device.device_id))
        {
            Ok(Pick::One(l)) => {
                let batch = samples(&l, &flows, unix_now());
                stored += batch.len() as u64;
                if let Err(e) = store.add(&device.name, &batch) {
                    warn!(device = %device.name, error = %e, "recorder: samples not stored");
                }
            }
            Ok(Pick::None) => {}
            Ok(Pick::Shared(n)) => {
                warn!(device = %device.name, devices = n, "recorder: address shared and no device id, nothing taken");
            }
            Err(e) => {
                warn!(device = %device.name, error = %e, "recorder: listen failed");
                std::thread::sleep(LISTEN);
            }
        }
    }
    quit.store(true, Ordering::Relaxed);
    info!(device = %device.name, samples = stored, "recorder: recording ended");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(slot: u16, peaks: &[u32], late: u64, total: u32) -> latency::SlotLatency {
        latency::SlotLatency {
            slot,
            peaks_us: peaks.to_vec(),
            late,
            late_total: total,
        }
    }

    fn heard(id: u8) -> latency::Latency {
        latency::Latency {
            device_id: [id; 8],
            beats: 1,
            rate: 48000,
            slots: vec![],
        }
    }

    #[test]
    fn pick_takes_a_single_device_and_refuses_a_shared_address_without_its_id() {
        assert_eq!(pick(vec![], None), Pick::None);
        assert_eq!(pick(vec![heard(1)], None), Pick::One(heard(1)));
        assert_eq!(pick(vec![heard(1), heard(2)], None), Pick::Shared(2));
    }

    #[test]
    fn pick_takes_the_device_by_its_id_at_a_shared_address() {
        assert_eq!(
            pick(vec![heard(1), heard(2)], Some([2; 8])),
            Pick::One(heard(2))
        );
        assert_eq!(pick(vec![heard(1)], Some([1; 8])), Pick::One(heard(1)));
        assert_eq!(pick(vec![], Some([1; 8])), Pick::None);
        // Only other devices' heartbeats at the address: none taken.
        assert_eq!(pick(vec![heard(3)], Some([1; 8])), Pick::Shared(1));
    }

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

    #[test]
    fn a_listen_becomes_one_sample_per_second_and_active_flow() {
        let active: BTreeMap<u16, FlowInfo> = [(
            1,
            FlowInfo {
                latency_us: 1000,
                ..FlowInfo::default()
            },
        )]
        .into();
        let l = latency::Latency {
            device_id: [0; 8],
            beats: 3,
            rate: 48000,
            slots: vec![slot(1, &[100, 200, 900], 2, 7), slot(5, &[0, 0, 0], 0, 0)],
        };
        let s = samples(&l, &active, 1_800_000_010);
        assert_eq!(
            s,
            vec![
                Sample {
                    slot: 1,
                    t: 1_800_000_008,
                    peak_us: 100,
                    late: 0
                },
                Sample {
                    slot: 1,
                    t: 1_800_000_009,
                    peak_us: 200,
                    late: 0
                },
                Sample {
                    slot: 1,
                    t: 1_800_000_010,
                    peak_us: 900,
                    late: 2
                },
            ]
        );
        assert!(samples(&l, &BTreeMap::new(), 1).is_empty());
    }
}
