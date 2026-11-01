//! Packet latency per receiver, as Dante Controller's Latency tab shows it.
//! A receiver says it in its heartbeat once a second (dstack `latency`);
//! the server listens to a device only while someone watches it: the
//! first request starts a watch, and a watch nobody asked for in [`IDLE`]
//! ends. The history (the per-second peaks of the last [`KEEP`] seconds)
//! stays on the server, so the page shows it at once after a reload.

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dante_stack::arc::client::{REPLY_TIMEOUT, Udp};
use dante_stack::arc::control;
use dante_stack::ctl::Requests;
use dante_stack::discover::Device;
use dante_stack::latency;
use dcontroller_api as api;
use tracing::{info, warn};

use crate::dante::{self, Shared};

/// A watch nobody asked for this long ends.
pub const IDLE: Duration = Duration::from_secs(30);
/// Seconds of peaks kept per flow.
pub const KEEP: usize = 600;
/// One listen: this many heartbeats' worth.
const LISTEN: Duration = Duration::from_secs(2);
/// How often the flows (latency, channels, sources) are read again.
const FLOWS_EVERY: Duration = Duration::from_secs(10);

/// One flow's history.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct History {
    pub peaks_us: Vec<u32>,
    pub late: u64,
    pub late_total: u32,
}

/// What a watch gathered about one device.
#[derive(Debug, Default)]
pub struct Watched {
    pub started: Option<Instant>,
    pub beats: u64,
    /// By slot.
    pub slots: BTreeMap<u16, History>,
    /// The flows as last read, by slot.
    pub flows: BTreeMap<u16, Flow>,
    pub error: Option<String>,
}

/// A flow's description.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flow {
    pub latency_us: u32,
    pub rate: u32,
    pub channels: Vec<String>,
    pub sources: Vec<String>,
}

impl Watched {
    /// Adds one listen's result.
    pub fn add(&mut self, l: &latency::Latency) {
        self.beats += u64::from(l.beats);
        for s in &l.slots {
            let h = self.slots.entry(s.slot).or_default();
            h.peaks_us.extend_from_slice(&s.peaks_us);
            if h.peaks_us.len() > KEEP {
                let cut = h.peaks_us.len() - KEEP;
                h.peaks_us.drain(..cut);
            }
            h.late += s.late;
            h.late_total = s.late_total;
        }
    }

    /// The report: the flows the device lists, with their history.
    pub fn report(&self, device: &str) -> api::LatencyReport {
        api::LatencyReport {
            device: device.to_owned(),
            watching_s: self.started.map_or(0, |s| s.elapsed().as_secs()),
            beats: self.beats,
            flows: self
                .flows
                .iter()
                .map(|(&slot, f)| {
                    let h = self.slots.get(&slot).cloned().unwrap_or_default();
                    api::FlowLatency {
                        slot,
                        latency_us: f.latency_us,
                        rate: f.rate,
                        channels: f.channels.clone(),
                        sources: f.sources.clone(),
                        peaks_us: h.peaks_us,
                        late: h.late,
                        late_total: h.late_total,
                    }
                })
                .collect(),
            error: self.error.clone(),
        }
    }
}

/// The flows of a device with their RX channel labels and sources.
pub fn describe(flows: &[control::RxFlow], rx: &[control::RxChannel]) -> BTreeMap<u16, Flow> {
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
                Flow {
                    latency_us: f.latency_ns / 1000,
                    rate: f.rate,
                    channels: fed.iter().map(|r| r.label.clone()).collect(),
                    sources,
                },
            )
        })
        .collect()
}

/// The start of the error for an address several devices share.
const SHARED: &str = "address shared";

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

/// One device heard: it; several: which is ours is unknown (two of our own
/// devices can share an address on their own ports).
pub fn pick(mut heard: Vec<latency::Latency>) -> Pick {
    match heard.len() {
        0 => Pick::None,
        1 => Pick::One(heard.remove(0)),
        n => Pick::Shared(n),
    }
}

struct Watch {
    asked: Instant,
    data: Arc<Mutex<Watched>>,
}

/// The watches, by device name (case aside).
#[derive(Default)]
pub struct Watches {
    map: Mutex<HashMap<String, Watch>>,
}

impl Watches {
    /// The report for `name`, starting a watch when none runs. A device not
    /// listed gives a report with the error.
    pub fn report(self: &Arc<Self>, dante: &Shared, name: &str) -> api::LatencyReport {
        let key = name.to_ascii_lowercase();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = map.get_mut(&key) {
            w.asked = Instant::now();
            return w
                .data
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .report(name);
        }
        let target = dante::State::target(&dante.read().unwrap_or_else(|e| e.into_inner()), name);
        let Some((ip, device)) = target else {
            return api::LatencyReport {
                device: name.to_owned(),
                error: Some(format!(
                    "{name} is not listed (discovery off or device gone)"
                )),
                ..api::LatencyReport::default()
            };
        };
        let data = Arc::new(Mutex::new(Watched {
            started: Some(Instant::now()),
            ..Watched::default()
        }));
        map.insert(
            key.clone(),
            Watch {
                asked: Instant::now(),
                data: Arc::clone(&data),
            },
        );
        drop(map);
        info!(device = %device.name, "latency: watch started");
        let me = Arc::clone(self);
        let report = data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .report(&device.name);
        std::thread::Builder::new()
            .name("latency".into())
            .spawn(move || me.run(&key, ip, &device, &data))
            .expect("spawn a latency thread");
        report
    }

    fn asked_lately(&self, key: &str) -> bool {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .is_some_and(|w| w.asked.elapsed() < IDLE)
    }

    fn run(&self, key: &str, ip: Ipv4Addr, device: &Device, data: &Mutex<Watched>) {
        let seq = AtomicU16::new(1);
        let requests = Requests::default();
        let stop = AtomicBool::new(false);
        let mut flows_read: Option<Instant> = None;
        while self.asked_lately(key) {
            if flows_read.is_none_or(|t| t.elapsed() >= FLOWS_EVERY) {
                flows_read = Some(Instant::now());
                let read = Udp::open(ip, device.arc, &seq, REPLY_TIMEOUT, &requests, &stop)
                    .map_err(|e| format!("socket: {e}"))
                    .and_then(|mut udp| {
                        let flows =
                            control::rx_flows(&mut udp).map_err(|e| format!("RX flows: {e}"))?;
                        let rx = control::rx_channels(&mut udp)
                            .map_err(|e| format!("RX channels: {e}"))?;
                        Ok(describe(&flows, &rx))
                    });
                let mut d = data.lock().unwrap_or_else(|e| e.into_inner());
                match read {
                    Ok(f) => {
                        d.flows = f;
                        d.error = None;
                    }
                    Err(e) => {
                        warn!(device = %device.name, error = %e, "latency: flows not read");
                        d.error = Some(e);
                    }
                }
            }
            match latency::listen(ip, *device.arc.ip(), LISTEN).map(pick) {
                Ok(Pick::One(l)) => {
                    let mut d = data.lock().unwrap_or_else(|e| e.into_inner());
                    d.add(&l);
                    if d.error.as_deref().is_some_and(|e| e.starts_with(SHARED)) {
                        d.error = None;
                    }
                }
                Ok(Pick::None) => {}
                Ok(Pick::Shared(n)) => {
                    let e = format!(
                        "{SHARED}: {n} devices send heartbeats from {}; dcontroller cannot tell which is {} yet",
                        device.arc.ip(),
                        device.name
                    );
                    warn!(device = %device.name, devices = n, "latency: address shared, nothing taken");
                    data.lock().unwrap_or_else(|e| e.into_inner()).error = Some(e);
                }
                Err(e) => {
                    warn!(device = %device.name, error = %e, "latency: listen failed");
                    data.lock().unwrap_or_else(|e| e.into_inner()).error =
                        Some(format!("listen: {e}"));
                    std::thread::sleep(LISTEN);
                }
            }
        }
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        let beats = data.lock().unwrap_or_else(|e| e.into_inner()).beats;
        info!(device = %device.name, beats, "latency: watch ended (nobody asked for {}s)", IDLE.as_secs());
    }
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
    fn pick_takes_a_single_device_and_refuses_a_shared_address() {
        assert_eq!(pick(vec![]), Pick::None);
        assert_eq!(pick(vec![heard(1)]), Pick::One(heard(1)));
        assert_eq!(pick(vec![heard(1), heard(2)]), Pick::Shared(2));
    }

    #[test]
    fn listens_add_up_and_keep_the_last_ten_minutes() {
        let mut w = Watched::default();
        w.add(&latency::Latency {
            device_id: [0; 8],
            beats: 2,
            rate: 48000,
            slots: vec![slot(1, &[100, 200], 0, 5)],
        });
        w.add(&latency::Latency {
            device_id: [0; 8],
            beats: 2,
            rate: 48000,
            slots: vec![slot(1, &[300, 900], 3, 8)],
        });
        assert_eq!(w.beats, 4);
        let h = &w.slots[&1];
        assert_eq!(h.peaks_us, [100, 200, 300, 900]);
        assert_eq!(h.late, 3);
        assert_eq!(h.late_total, 8);
        let many: Vec<u32> = (0..KEEP as u32 + 5).collect();
        w.add(&latency::Latency {
            device_id: [0; 8],
            beats: 1,
            rate: 48000,
            slots: vec![slot(1, &many, 0, 8)],
        });
        let h = &w.slots[&1];
        assert_eq!(h.peaks_us.len(), KEEP);
        assert_eq!(*h.peaks_us.last().unwrap(), KEEP as u32 + 4);
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
    fn the_report_lists_described_flows_with_their_history() {
        let mut w = Watched::default();
        w.add(&latency::Latency {
            device_id: [0; 8],
            beats: 1,
            rate: 48000,
            slots: vec![slot(1, &[400], 1, 1), slot(7, &[0], 0, 0)],
        });
        w.flows.insert(
            1,
            Flow {
                latency_us: 1000,
                rate: 48000,
                channels: vec!["In 1".into()],
                sources: vec!["desk".into()],
            },
        );
        let r = w.report("stage");
        assert_eq!(r.device, "stage");
        assert_eq!(r.beats, 1);
        assert_eq!(r.flows.len(), 1);
        assert_eq!(r.flows[0].peaks_us, [400]);
        assert_eq!(r.flows[0].late, 1);
        assert_eq!(r.flows[0].latency_us, 1000);
    }

    #[test]
    fn an_unlisted_device_reports_why_and_starts_nothing() {
        let w = Arc::new(Watches::default());
        let off: Shared = Arc::new(std::sync::RwLock::new(dante::State::off(None, None)));
        let r = w.report(&off, "stage");
        assert!(r.error.unwrap().contains("not listed"));
        assert!(w.map.lock().unwrap().is_empty());
    }
}
