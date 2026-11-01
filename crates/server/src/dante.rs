//! The Dante side: device discovery and read-only channel reads through
//! dstack (`dante_stack`). Nothing here writes to any device: no write call
//! of `arc::control` is used.
//!
//! One thread owns discovery and the reads. A device is read when it is
//! listed, when its records change, and every [`REFRESH`] after its last
//! read. The HTTP side only reads the shared [`State`].

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dante_stack::arc::client::{REPLY_TIMEOUT, ReadError, Udp};
use dante_stack::arc::control::{self, Call};
use dante_stack::ctl::Requests;
use dante_stack::discover::{Change, Device, Discovery};
use dcontroller_api as api;
use tracing::{info, warn};

/// How often a listed device is read again.
pub const REFRESH: Duration = Duration::from_secs(30);
/// How long the thread waits for a discovery change before it looks for due reads.
const TICK: Duration = Duration::from_millis(500);

/// Discovery's table plus each device's last read.
#[derive(Debug, Default)]
pub struct State {
    enabled: bool,
    /// Our address on the Dante interface while discovery runs.
    ip: Option<Ipv4Addr>,
    interface: Option<String>,
    error: Option<String>,
    devices: BTreeMap<String, Entry>,
}

#[derive(Debug)]
struct Entry {
    device: Device,
    channels: Option<api::Channels>,
    read_at: Option<u64>,
    error: Option<String>,
    /// When the next read is due; `None` = now.
    due: Option<Instant>,
}

/// The state the HTTP side reads.
pub type Shared = Arc<RwLock<State>>;

impl State {
    /// Discovery is off: `error` says why (`None` when no interface is configured).
    pub fn off(interface: Option<String>, error: Option<String>) -> Self {
        State {
            enabled: false,
            ip: None,
            interface,
            error,
            devices: BTreeMap::new(),
        }
    }

    /// Discovery runs on `interface`, from our address `ip`.
    pub fn on(interface: String, ip: Ipv4Addr) -> Self {
        State {
            enabled: true,
            ip: Some(ip),
            interface: Some(interface),
            error: None,
            devices: BTreeMap::new(),
        }
    }

    /// Applies one discovery change. A new or changed device is due for a read.
    pub fn apply(&mut self, change: Change) {
        match change {
            Change::Added(device) | Change::Changed(device) => {
                let key = device.name.to_ascii_lowercase();
                match self.devices.get_mut(&key) {
                    Some(e) => {
                        e.device = device;
                        e.due = None;
                    }
                    None => {
                        self.devices.insert(
                            key,
                            Entry {
                                device,
                                channels: None,
                                read_at: None,
                                error: None,
                                due: None,
                            },
                        );
                    }
                }
            }
            Change::Removed(name) => {
                self.devices.remove(&name.to_ascii_lowercase());
            }
        }
    }

    /// The devices whose read is due at `now`.
    pub fn due(&self, now: Instant) -> Vec<Device> {
        self.devices
            .values()
            .filter(|e| e.due.is_none_or(|d| d <= now))
            .map(|e| e.device.clone())
            .collect()
    }

    /// Stores a read of `device` (still listed under that name) made at
    /// `now`; the next is due [`REFRESH`] later. A failed read keeps the
    /// previous channels and records the error.
    pub fn record(
        &mut self,
        device: &Device,
        result: Result<api::Channels, String>,
        now: Instant,
        unix: u64,
    ) {
        let Some(e) = self.devices.get_mut(&device.name.to_ascii_lowercase()) else {
            return;
        };
        if e.device != *device {
            // Its records changed during the read: the change already made it due.
            return;
        }
        e.due = Some(now + REFRESH);
        match result {
            Ok(ch) => {
                e.channels = Some(ch);
                e.read_at = Some(unix);
                e.error = None;
            }
            Err(err) => e.error = Some(err),
        }
    }

    /// Our address and the ARC of the listed device `name` (case aside),
    /// with its listed name: where a change goes.
    pub fn target(&self, name: &str) -> Option<(Ipv4Addr, Device)> {
        let ip = self.ip?;
        let e = self.devices.get(&name.to_ascii_lowercase())?;
        Some((ip, e.device.clone()))
    }

    /// Makes `device` due for a read now (after a change to it).
    pub fn mark_due(&mut self, device: &Device) {
        if let Some(e) = self.devices.get_mut(&device.name.to_ascii_lowercase()) {
            e.due = None;
        }
    }

    /// Replaces the RX channels of `device` (still listed with the same
    /// records) with a fresh read, as after a change.
    pub fn set_rx(&mut self, device: &Device, rx: Vec<api::RxChannel>) {
        let Some(e) = self.devices.get_mut(&device.name.to_ascii_lowercase()) else {
            return;
        };
        if e.device != *device {
            return;
        }
        if let Some(c) = e.channels.as_mut() {
            c.rx = rx;
        }
    }

    /// The JSON view.
    pub fn view(&self) -> api::Devices {
        api::Devices {
            enabled: self.enabled,
            interface: self.interface.clone(),
            error: self.error.clone(),
            devices: self
                .devices
                .values()
                .map(|e| api::Device {
                    name: e.device.name.clone(),
                    address: e.device.arc.to_string(),
                    manufacturer: e.device.txt.get("mf").cloned(),
                    model: model(&e.device),
                    channels: e.channels.clone(),
                    read_at: e.read_at,
                    error: e.error.clone(),
                })
                .collect(),
        }
    }
}

/// The model name to show: mDNS `router_info` (the board name, readable),
/// else `model` (often a Dante model id such as `_000000000000000b`).
fn model(d: &Device) -> Option<String> {
    d.txt
        .get("router_info")
        .or_else(|| d.txt.get("model"))
        .filter(|m| !m.is_empty())
        .cloned()
}

/// The subscription's status in Dante Controller's words (its code in hex
/// when dstack does not know it) and its state for the colours; `None` when
/// the channel is not subscribed.
pub fn status_text(rx: &control::RxChannel) -> Option<(String, Option<String>)> {
    use dante_stack::subscribe::SubState;
    rx.tx_channel.as_ref()?;
    Some(match rx.sub_status() {
        Some(s) => {
            let state = match s.state() {
                SubState::Idle => "idle",
                SubState::Pending => "pending",
                SubState::Connected => "connected",
                SubState::Error => "error",
            };
            (s.label().to_owned(), Some(state.to_owned()))
        }
        None => (format!("0x{:x}", rx.status), None),
    })
}

/// A device's channels: TX labels and RX channels with their subscriptions.
/// Read-only: two read requests' worth of pages.
pub fn read_channels(c: &mut impl Call) -> Result<api::Channels, ReadError> {
    let labels = control::labels(c)?;
    let rx = control::rx_channels(c)?;
    Ok(api::Channels {
        tx: labels
            .tx
            .into_iter()
            .map(|(channel, label)| api::TxChannel { channel, label })
            .collect(),
        rx: rx_view(&rx),
    })
}

/// The RX channels as the API shows them.
pub fn rx_view(rx: &[control::RxChannel]) -> Vec<api::RxChannel> {
    rx.iter()
        .map(|r| api::RxChannel {
            channel: r.channel,
            label: r.label.clone(),
            source: match (&r.tx_channel, &r.tx_device) {
                (Some(c), Some(d)) => Some(api::Source {
                    channel: c.clone(),
                    device: d.clone(),
                }),
                _ => None,
            },
            status: status_text(r).map(|(t, _)| t),
            state: status_text(r).and_then(|(_, s)| s),
        })
        .collect()
}

/// The IPv4 address of `iface`.
pub fn interface_ipv4(iface: &str) -> Result<Ipv4Addr, String> {
    let addrs = if_addrs::get_if_addrs().map_err(|e| format!("list interfaces: {e}"))?;
    addrs
        .iter()
        .filter(|a| a.name == iface)
        .find_map(|a| match a.ip() {
            std::net::IpAddr::V4(ip) => Some(ip),
            std::net::IpAddr::V6(_) => None,
        })
        .ok_or_else(|| format!("interface {iface} has no IPv4 address"))
}

/// Starts discovery and the reads on `iface` in a thread of their own, and
/// returns the shared state. The thread waits for the interface's IPv4
/// (a USB NIC may come up after the service), restarts discovery when that
/// address changes or discovery ends, and says why in the state meanwhile.
pub fn start(iface: Option<String>, stop: Arc<AtomicBool>) -> Shared {
    let Some(iface) = iface else {
        info!("dante: no dante_interface configured, discovery off");
        return Arc::new(RwLock::new(State::off(None, None)));
    };
    let shared = Arc::new(RwLock::new(State::off(
        Some(iface.clone()),
        Some(format!("waiting for {iface}")),
    )));
    let state = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("dante".into())
        .spawn(move || supervise(&iface, &state, &stop))
        .expect("spawn the dante thread");
    shared
}

/// How long the thread waits before it tries the interface or discovery again.
const RETRY: Duration = Duration::from_secs(5);
/// How often a running discovery checks that the interface keeps its address.
const ADDRESS_CHECK: Duration = Duration::from_secs(10);

fn supervise(iface: &str, state: &Shared, stop: &Arc<AtomicBool>) {
    let mut last_error = String::new();
    let mut off = |e: String| {
        if e != last_error {
            warn!(interface = %iface, error = %e, "dante: discovery off, retrying every {}s", RETRY.as_secs());
            last_error.clone_from(&e);
        }
        *write(state) = State::off(Some(iface.to_owned()), Some(e));
    };
    while !stop.load(Ordering::Relaxed) {
        let ip = match interface_ipv4(iface) {
            Ok(ip) => ip,
            Err(e) => {
                off(e);
                sleep_unless_stopped(RETRY, stop);
                continue;
            }
        };
        let (discovery, changes) = match Discovery::start(ip) {
            Ok(d) => d,
            Err(e) => {
                off(format!("discovery did not start on {ip}: {e}"));
                sleep_unless_stopped(RETRY, stop);
                continue;
            }
        };
        info!(interface = %iface, %ip, "dante: discovery started");
        *write(state) = State::on(iface.to_owned(), ip);
        let ended = run(iface, ip, discovery, changes, state, stop);
        if let Some(why) = ended {
            off(why);
            sleep_unless_stopped(RETRY, stop);
        }
    }
    info!("dante: stopped");
}

fn sleep_unless_stopped(d: Duration, stop: &AtomicBool) {
    let until = Instant::now() + d;
    while !stop.load(Ordering::Relaxed) && Instant::now() < until {
        std::thread::sleep(TICK);
    }
}

/// Runs discovery and the reads until `stop` (returns `None`) or until
/// discovery ends or the interface's address changes (returns why).
fn run(
    iface: &str,
    ip: Ipv4Addr,
    discovery: Discovery,
    changes: std::sync::mpsc::Receiver<Change>,
    state: &Shared,
    stop: &AtomicBool,
) -> Option<String> {
    let seq = AtomicU16::new(1);
    let requests = Requests::default();
    let mut checked = Instant::now();
    let mut ended = None;
    while !stop.load(Ordering::Relaxed) {
        if checked.elapsed() >= ADDRESS_CHECK {
            checked = Instant::now();
            match interface_ipv4(iface) {
                Ok(now) if now == ip => {}
                Ok(now) => {
                    ended = Some(format!("{iface} changed address from {ip} to {now}"));
                    break;
                }
                Err(e) => {
                    ended = Some(e);
                    break;
                }
            }
        }
        match changes.recv_timeout(TICK) {
            Ok(change) => {
                match &change {
                    Change::Added(d) => {
                        info!(device = %d.name, arc = %d.arc, "dante: device added")
                    }
                    Change::Changed(d) => {
                        info!(device = %d.name, arc = %d.arc, "dante: device changed")
                    }
                    Change::Removed(n) => info!(device = %n, "dante: device removed"),
                }
                write(state).apply(change);
                continue; // take every queued change before reading
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                ended = Some("discovery ended".to_owned());
                break;
            }
        }
        let due = read(state).due(Instant::now());
        for device in due {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let started = Instant::now();
            let result = Udp::open(ip, device.arc, &seq, REPLY_TIMEOUT, &requests, stop)
                .map_err(|e| format!("socket: {e}"))
                .and_then(|mut udp| read_channels(&mut udp).map_err(|e| e.to_string()));
            match &result {
                Ok(ch) => info!(device = %device.name, tx = ch.tx.len(), rx = ch.rx.len(),
                    ms = started.elapsed().as_millis() as u64, "dante: channels read"),
                Err(e) => warn!(device = %device.name, error = %e, "dante: channel read failed"),
            }
            let unix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            write(state).record(&device, result, Instant::now(), unix);
        }
    }
    if let Err(e) = discovery.stop() {
        warn!(error = %e, "dante: discovery stop");
    }
    info!(interface = %iface, %ip, why = ?ended, "dante: discovery stopped");
    ended
}

fn read(s: &Shared) -> std::sync::RwLockReadGuard<'_, State> {
    s.read().unwrap_or_else(|e| e.into_inner())
}

fn write(s: &Shared) -> std::sync::RwLockWriteGuard<'_, State> {
    s.write().unwrap_or_else(|e| e.into_inner())
}

/// The JSON view of `s`.
pub fn snapshot(s: &Shared) -> api::Devices {
    read(s).view()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV4;

    fn dev(name: &str, last: u8) -> Device {
        let mut txt = BTreeMap::new();
        txt.insert("mf".into(), "Acme".into());
        txt.insert("model".into(), "Box".into());
        Device {
            name: name.into(),
            arc: SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, last), 4440),
            dbc: None,
            device_id: None,
            txt,
        }
    }

    fn channels(label: &str) -> api::Channels {
        api::Channels {
            tx: vec![api::TxChannel {
                channel: 1,
                label: label.into(),
            }],
            rx: vec![],
        }
    }

    #[test]
    fn added_device_is_listed_and_due_at_once() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        let now = Instant::now();
        assert_eq!(s.due(now), vec![dev("Desk", 1)]);
        let v = s.view();
        assert!(v.enabled);
        assert_eq!(v.devices.len(), 1);
        assert_eq!(v.devices[0].name, "Desk");
        assert_eq!(v.devices[0].address, "10.0.0.1:4440");
        assert_eq!(v.devices[0].manufacturer.as_deref(), Some("Acme"));
        assert_eq!(v.devices[0].model.as_deref(), Some("Box"));
        // The readable board name wins over a model id.
        let mut d = dev("Desk", 1);
        d.txt.insert("router_info".into(), "Brooklyn II".into());
        s.apply(Change::Changed(d));
        assert_eq!(s.view().devices[0].model.as_deref(), Some("Brooklyn II"));
        assert!(v.devices[0].channels.is_none());
    }

    #[test]
    fn a_read_is_stored_and_the_next_is_due_after_refresh() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        let now = Instant::now();
        s.record(&dev("Desk", 1), Ok(channels("Vox")), now, 42);
        assert!(s.due(now).is_empty());
        assert!(s.due(now + REFRESH - Duration::from_millis(1)).is_empty());
        assert_eq!(s.due(now + REFRESH).len(), 1);
        let v = s.view();
        assert_eq!(v.devices[0].channels, Some(channels("Vox")));
        assert_eq!(v.devices[0].read_at, Some(42));
    }

    #[test]
    fn a_failed_read_keeps_the_previous_channels_and_shows_the_error() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        let now = Instant::now();
        s.record(&dev("Desk", 1), Ok(channels("Vox")), now, 42);
        s.record(&dev("Desk", 1), Err("no reply".into()), now + REFRESH, 99);
        let v = s.view();
        assert_eq!(v.devices[0].channels, Some(channels("Vox")));
        assert_eq!(v.devices[0].read_at, Some(42));
        assert_eq!(v.devices[0].error.as_deref(), Some("no reply"));
        s.record(
            &dev("Desk", 1),
            Ok(channels("Bass")),
            now + REFRESH * 2,
            100,
        );
        assert_eq!(s.view().devices[0].error, None);
    }

    #[test]
    fn changed_records_make_it_due_and_drop_a_read_of_the_old_records() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        let now = Instant::now();
        s.record(&dev("Desk", 1), Ok(channels("Vox")), now, 42);
        s.apply(Change::Changed(dev("Desk", 2)));
        assert_eq!(s.due(now), vec![dev("Desk", 2)]);
        // A read that started before the change lands after it: ignored.
        s.record(&dev("Desk", 1), Ok(channels("Old")), now, 43);
        assert_eq!(s.view().devices[0].channels, Some(channels("Vox")));
        assert_eq!(s.view().devices[0].address, "10.0.0.2:4440");
    }

    #[test]
    fn removed_device_leaves_the_list_case_aside() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        s.apply(Change::Added(dev("amp", 3)));
        s.apply(Change::Removed("DESK".into()));
        let v = s.view();
        assert_eq!(v.devices.len(), 1);
        assert_eq!(v.devices[0].name, "amp");
        // A read of a removed device is dropped.
        s.record(&dev("Desk", 1), Ok(channels("Vox")), Instant::now(), 1);
        assert_eq!(s.view().devices.len(), 1);
    }

    #[test]
    fn list_is_sorted_by_name_case_aside() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("zeta", 1)));
        s.apply(Change::Added(dev("Alpha", 2)));
        s.apply(Change::Added(dev("beta", 3)));
        let names: Vec<_> = s.view().devices.into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["Alpha", "beta", "zeta"]);
    }

    #[test]
    fn off_state_says_why() {
        let v = State::off(
            Some("eth9".into()),
            Some("interface eth9 has no IPv4 address".into()),
        )
        .view();
        assert!(!v.enabled);
        assert_eq!(v.interface.as_deref(), Some("eth9"));
        assert!(v.error.unwrap().contains("eth9"));
        assert!(v.devices.is_empty());
    }

    #[test]
    fn a_missing_interface_is_waited_for_and_the_thread_stops() {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = start(Some("no-such-iface0".into()), Arc::clone(&stop));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let v = snapshot(&shared);
            if v.error
                .as_deref()
                .is_some_and(|e| e.contains("has no IPv4"))
            {
                assert!(!v.enabled);
                assert_eq!(v.interface.as_deref(), Some("no-such-iface0"));
                break;
            }
            assert!(Instant::now() < deadline, "{v:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn missing_interface_is_an_error() {
        let e = interface_ipv4("no-such-iface0").unwrap_err();
        assert!(e.contains("no-such-iface0"), "{e}");
        assert!(interface_ipv4("lo").unwrap().is_loopback());
    }

    #[test]
    fn status_text_names_known_codes_and_only_for_subscribed_channels() {
        let mut rx = control::RxChannel {
            channel: 1,
            label: "In".into(),
            tx_channel: None,
            tx_device: None,
            status: 0,
        };
        assert_eq!(status_text(&rx), None);
        rx.tx_channel = Some("Vox".into());
        rx.tx_device = Some("desk".into());
        // Seen on site: status 9 with an unknown receiver detail.
        rx.status = 0x0301_0009;
        let (text, state) = status_text(&rx).unwrap();
        assert!(text.starts_with("Subscribed"), "{text}");
        assert_eq!(state.as_deref(), Some("connected"));
        rx.status = 4;
        assert_eq!(status_text(&rx).unwrap().0, "Subscribed (self)");
        rx.status = 1;
        let (text, state) = status_text(&rx).unwrap();
        assert_ne!(state.as_deref(), Some("connected"), "{text}");
        rx.status = 0x77;
        assert_eq!(status_text(&rx), Some(("0x77".to_owned(), None)));
    }
}
