//! Channel meters, as Dante Controller's meters show them. A device sends
//! its levels while a controller holds a meter subscription on its CMC
//! port (dstack `meter`); the server holds one only while someone watches
//! the device: the first request starts a watch, and a watch nobody asked
//! for in [`IDLE`] sends the stop and ends. Frames that stop for [`STALE`]
//! renew the subscription.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dante_stack::cmc;
use dante_stack::discover::Device;
use dante_stack::meter::{self, Frame, Level, Meter};
use dcontroller_api as api;
use tracing::{info, warn};

use crate::dante::Shared;

/// A watch nobody asked for this long ends.
pub const IDLE: Duration = Duration::from_secs(15);
/// No frame for this long renews the subscription.
pub const STALE: Duration = Duration::from_secs(5);
/// How long the watch waits for a frame before it looks around.
const TICK: Duration = Duration::from_millis(500);
/// The controller name the device sees.
pub const NAME: &str = "dcontroller";

/// The levels of `raw` (one byte per channel) with the channels' labels by
/// number (from 1); a channel without a label shows its number.
pub fn levels(raw: &[u8], labels: &[(u16, String)]) -> Vec<api::ChannelLevel> {
    raw.iter()
        .enumerate()
        .filter_map(|(i, &b)| {
            let channel = u16::try_from(i + 1).ok()?;
            let label = labels
                .iter()
                .find(|(c, _)| *c == channel)
                .map_or_else(|| channel.to_string(), |(_, l)| l.clone());
            let (dbfs, clip, muted) = match meter::level(b) {
                Level::Clipping => (Some(0.0), true, false),
                Level::Dbfs(d) => (Some(d as f32), false, false),
                Level::Muted => (None, false, true),
                Level::Marker => (None, false, false),
            };
            Some(api::ChannelLevel {
                channel,
                label,
                dbfs,
                clip,
                muted,
            })
        })
        .collect()
}

/// Whether `frame` is the watched device's: its device id when discovery
/// has one (several devices can share an address), else any frame from
/// its address.
pub fn ours(frame: &Frame, id: Option<[u8; 8]>) -> bool {
    id.is_none_or(|id| frame.device_id == id)
}

/// A MAC as `/sys/class/net/<iface>/address` writes it.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let parts: Vec<u8> = text
        .trim()
        .split(':')
        .map(|p| u8::from_str_radix(p, 16).ok())
        .collect::<Option<_>>()?;
    parts.try_into().ok()
}

fn mac_of(iface: &str) -> Result<[u8; 6], String> {
    let path = format!("/sys/class/net/{iface}/address");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {path}: {e}"))?;
    parse_mac(&text).ok_or_else(|| format!("{path}: not a MAC: {}", text.trim()))
}

/// What a watch gathered.
#[derive(Debug, Default)]
struct Watched {
    last: Option<(Instant, Frame)>,
    frames: u64,
    error: Option<String>,
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
    /// The levels of `name`, starting a watch when none runs.
    pub fn report(self: &Arc<Self>, dante: &Shared, name: &str) -> api::MeterReport {
        let (target, iface, channels) = {
            let st = dante.read().unwrap_or_else(|e| e.into_inner());
            (st.target(name), st.interface(), st.channels(name))
        };
        let labels = |dir: fn(&api::Channels) -> Vec<(u16, String)>| {
            channels.as_ref().map(dir).unwrap_or_default()
        };
        let tx = labels(|c| c.tx.iter().map(|t| (t.channel, t.label.clone())).collect());
        let rx = labels(|c| c.rx.iter().map(|r| (r.channel, r.label.clone())).collect());
        let view = |d: &Watched| api::MeterReport {
            device: name.to_owned(),
            age_ms: d
                .last
                .as_ref()
                .map(|(t, _)| u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)),
            frames: d.frames,
            tx: d
                .last
                .as_ref()
                .map(|(_, f)| levels(&f.tx, &tx))
                .unwrap_or_default(),
            rx: d
                .last
                .as_ref()
                .map(|(_, f)| levels(&f.rx, &rx))
                .unwrap_or_default(),
            error: d.error.clone(),
        };
        let key = name.to_ascii_lowercase();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = map.get_mut(&key) {
            w.asked = Instant::now();
            return view(&w.data.lock().unwrap_or_else(|e| e.into_inner()));
        }
        let (Some((ip, device)), Some(iface)) = (target, iface) else {
            return api::MeterReport {
                device: name.to_owned(),
                error: Some(format!(
                    "{name} is not listed (discovery off or device gone)"
                )),
                ..api::MeterReport::default()
            };
        };
        let data = Arc::new(Mutex::new(Watched::default()));
        map.insert(
            key.clone(),
            Watch {
                asked: Instant::now(),
                data: Arc::clone(&data),
            },
        );
        drop(map);
        info!(device = %device.name, "meters: watch started");
        let me = Arc::clone(self);
        let report = view(&data.lock().unwrap_or_else(|e| e.into_inner()));
        std::thread::Builder::new()
            .name("meters".into())
            .spawn(move || me.run(&key, ip, &iface, &device, &data))
            .expect("spawn a meters thread");
        report
    }

    fn asked_lately(&self, key: &str) -> bool {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .is_some_and(|w| w.asked.elapsed() < IDLE)
    }

    fn run(&self, key: &str, ip: Ipv4Addr, iface: &str, device: &Device, data: &Mutex<Watched>) {
        let at = SocketAddrV4::new(*device.arc.ip(), cmc::PORT);
        let set_error = |e: String| {
            warn!(device = %device.name, error = %e, "meters: no subscription");
            data.lock().unwrap_or_else(|e| e.into_inner()).error = Some(e);
        };
        let mut sub: Option<(Meter, std::sync::mpsc::Receiver<Frame>, Instant)> = None;
        while self.asked_lately(key) {
            let Some((_, frames, since)) = sub.as_mut() else {
                match mac_of(iface).and_then(|mac| {
                    Meter::start(ip, at, NAME, mac).map_err(|e| format!("subscribe: {e}"))
                }) {
                    Ok((m, rx)) => sub = Some((m, rx, Instant::now())),
                    Err(e) => {
                        set_error(e);
                        std::thread::sleep(STALE);
                    }
                }
                continue;
            };
            match frames.recv_timeout(TICK) {
                Ok(f) if ours(&f, device.device_id) => {
                    *since = Instant::now();
                    let mut d = data.lock().unwrap_or_else(|e| e.into_inner());
                    d.frames += 1;
                    d.error = None;
                    d.last = Some((Instant::now(), f));
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    set_error("the meter listener ended".into());
                    sub = None;
                    continue;
                }
            }
            if since.elapsed() >= STALE {
                let mut d = data.lock().unwrap_or_else(|e| e.into_inner());
                if d.error.is_none() {
                    d.error = Some(format!(
                        "no meter frames from {} for {} s: subscription renewed",
                        device.name,
                        STALE.as_secs()
                    ));
                }
                drop(d);
                info!(device = %device.name, "meters: no frames, renewing the subscription");
                if let Some((m, _, _)) = sub.take()
                    && let Err(e) = m.stop()
                {
                    warn!(device = %device.name, error = %e, "meters: stop not sent");
                }
            }
        }
        if let Some((m, _, _)) = sub
            && let Err(e) = m.stop()
        {
            warn!(device = %device.name, error = %e, "meters: stop not sent");
        }
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
        let frames = data.lock().unwrap_or_else(|e| e.into_inner()).frames;
        info!(device = %device.name, frames, "meters: watch ended (nobody asked for {}s)", IDLE.as_secs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_read_the_meter_scale_with_the_channel_labels() {
        let labels = vec![(1, "Mix L".to_owned()), (2, "Mix R".to_owned())];
        let l = levels(&[0, 1, 41, 254, 255], &labels);
        assert_eq!(l.len(), 5);
        assert_eq!(
            (l[0].label.as_str(), l[0].dbfs, l[0].clip),
            ("Mix L", Some(0.0), true)
        );
        assert_eq!(
            (l[1].label.as_str(), l[1].dbfs, l[1].clip),
            ("Mix R", Some(0.0), false)
        );
        assert_eq!(l[2].dbfs, Some(-20.0));
        assert_eq!(l[2].label, "3");
        assert_eq!((l[3].dbfs, l[3].muted), (None, true));
        assert_eq!((l[4].dbfs, l[4].muted, l[4].clip), (None, false, false));
        assert_eq!(l[4].channel, 5);
    }

    #[test]
    fn frames_are_taken_by_device_id_when_discovery_has_one() {
        let f = Frame {
            device_id: [7; 8],
            tx: vec![],
            rx: vec![],
        };
        assert!(ours(&f, Some([7; 8])));
        assert!(!ours(&f, Some([8; 8])));
        assert!(ours(&f, None));
    }

    #[test]
    fn macs_read_as_sysfs_writes_them() {
        assert_eq!(
            parse_mac("6c:1f:f7:c5:df:69\n"),
            Some([0x6c, 0x1f, 0xf7, 0xc5, 0xdf, 0x69])
        );
        assert_eq!(parse_mac("6c:1f:f7"), None);
        assert_eq!(parse_mac("zz:1f:f7:c5:df:69"), None);
    }
}
