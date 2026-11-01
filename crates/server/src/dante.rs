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

use dante_stack::arc::client::{Exchange, REPLY_TIMEOUT, ReadError, Udp, read_directory};
use dante_stack::arc::control::{self, Call};
use dante_stack::ctl::Requests;
use dante_stack::discover::{Change, Device, Discovery};
use dante_stack::info::client::{self as info_client, Board, ClockStatus, NetInterface, Product};
use dcontroller_api as api;
use tracing::{info, warn};

/// How often a listed device is read again.
pub const REFRESH: Duration = Duration::from_secs(30);
/// How long the thread waits for a discovery change before it looks for due reads.
const TICK: Duration = Duration::from_millis(500);
/// How long a product or board info ask waits for its reply.
const INFO_WAIT: Duration = Duration::from_secs(1);

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
    settings: Option<api::Settings>,
    /// Read once per listing (again when its records change).
    info: Option<api::Info>,
    /// The last clock read: the status, or why there is none.
    clock: Option<Result<api::Clock, String>>,
    /// The last network read: the interfaces, or why there are none.
    network: Option<Result<Vec<api::NetPort>, String>>,
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
                        e.info = None;
                        e.due = None;
                    }
                    None => {
                        self.devices.insert(
                            key,
                            Entry {
                                device,
                                channels: None,
                                settings: None,
                                info: None,
                                clock: None,
                                network: None,
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
        result: Result<Read, String>,
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
            Ok(r) => {
                e.channels = Some(r.channels);
                // A device that does not answer the settings read keeps what it last said.
                if r.settings.is_some() {
                    e.settings = r.settings;
                }
                if r.info.is_some() {
                    e.info = r.info;
                }
                if r.clock.is_some() {
                    e.clock = r.clock;
                }
                if r.network.is_some() {
                    e.network = r.network;
                }
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

    /// Whether `device`'s product info is still to be read.
    pub fn needs_info(&self, device: &Device) -> bool {
        self.devices
            .get(&device.name.to_ascii_lowercase())
            .is_some_and(|e| e.info.is_none())
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
                    manufacturer: e
                        .info
                        .as_ref()
                        .and_then(|i| i.manufacturer.clone())
                        .or_else(|| e.device.txt.get("mf").cloned()),
                    model: e
                        .info
                        .as_ref()
                        .and_then(|i| i.model.clone())
                        .or_else(|| model(&e.device)),
                    channels: e.channels.clone(),
                    settings: e.settings.clone(),
                    info: e.info.clone(),
                    clock: e.clock.clone().and_then(Result::ok),
                    clock_error: e.clock.clone().and_then(Result::err),
                    network: e.network.clone().and_then(Result::ok),
                    network_error: e.network.clone().and_then(Result::err),
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

/// One read of a device: its channels and, when it answers, its settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Read {
    pub channels: api::Channels,
    pub settings: Option<api::Settings>,
    /// `None`: not read this time (the last one stays).
    pub info: Option<api::Info>,
    /// `None`: not read this time (the last one stays).
    pub clock: Option<Result<api::Clock, String>>,
    /// `None`: not read this time (the last one stays).
    pub network: Option<Result<Vec<api::NetPort>, String>>,
}

/// The network interfaces as the API shows them.
pub fn network_view(ports: &[NetInterface]) -> Vec<api::NetPort> {
    ports
        .iter()
        .map(|p| api::NetPort {
            link_mbit: p.link_mbit,
            mac: mac(&p.mac),
            ip: p.ip.to_string(),
            netmask: p.netmask.to_string(),
            gateway: p.gateway.to_string(),
            dns: p.dns.to_string(),
        })
        .collect()
}

/// Asks `device` for its network interfaces from our address `our` (UDP
/// 8700, read-only).
pub fn read_network(our: Ipv4Addr, device: &Device) -> Result<Vec<api::NetPort>, String> {
    info_client::network_info(our, *device.arc.ip(), device.device_id, INFO_WAIT)
        .map(|n| network_view(&n))
        .map_err(|e| e.to_string())
}

/// A clock id as a MAC.
fn mac(id: &[u8; 6]) -> String {
    id.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The clock status as the API shows it.
pub fn clock_view(c: &ClockStatus) -> api::Clock {
    let named = |name: Option<&str>, what: &str, code: u16| {
        name.map_or_else(|| format!("{what} {code}"), str::to_owned)
    };
    api::Clock {
        leader: c.is_leader(),
        grandmaster: c.is_grandmaster(),
        state: named(c.clock_state_name(), "state", c.clock_state),
        sync: named(c.servo_state_name(), "servo", c.servo_state),
        stratum: c.stratum,
        frequency_ppb: c.frequency_ppb,
        max_drift_ppb: c.max_drift_ppb,
        id: mac(&c.device),
        master: mac(&c.master),
        grandmaster_id: mac(&c.grandmaster),
        ports: c
            .ports
            .iter()
            .map(|&p| named(info_client::port_state_name(p), "state", p))
            .collect(),
        word_clock: c.word_clock.map(|w| match w {
            0 => "unknown".to_owned(),
            1 => "none".to_owned(),
            3 => "valid".to_owned(),
            n => format!("state {n}"),
        }),
    }
}

/// Asks `device` for its clock status from our address `our` (UDP 8700,
/// read-only).
pub fn read_clock(our: Ipv4Addr, device: &Device) -> Result<api::Clock, String> {
    info_client::clock_status(our, *device.arc.ip(), device.device_id, INFO_WAIT)
        .map(|c| clock_view(&c))
        .map_err(|e| e.to_string())
}

/// What the device says it is, from the replies it gave; `None` when it
/// gave none.
pub fn info_view(
    product: Option<&Product>,
    board: Option<&Board>,
    encoding: Option<u32>,
) -> Option<api::Info> {
    if product.is_none() && board.is_none() && encoding.is_none() {
        return None;
    }
    let text = |s: &String| (!s.is_empty()).then(|| s.clone());
    let ver = |v: Option<info_client::Version>| v.map(|v| v.to_string());
    Some(api::Info {
        manufacturer: product.and_then(|p| text(&p.manufacturer)),
        model: product.and_then(|p| text(&p.model)),
        product_version: product.and_then(|p| ver(p.version)),
        software_version: product.and_then(|p| ver(p.software)),
        firmware_version: product.and_then(|p| ver(p.firmware)),
        platform: board.and_then(|b| text(&b.model).or_else(|| text(&b.board))),
        platform_software: board.and_then(|b| ver(b.software)),
        platform_hardware: board.and_then(|b| ver(b.hardware)),
        encoding_bits: encoding,
    })
}

/// Asks `device` for its product and board info from our address `our`
/// (UDP 8700, read-only) and reads its encoding through `ex`. A part that
/// does not answer is logged and left out.
pub fn read_info(our: Ipv4Addr, device: &Device, ex: &mut impl Exchange) -> Option<api::Info> {
    let ip = *device.arc.ip();
    let product = info_client::product_info(our, ip, device.device_id, INFO_WAIT)
        .inspect_err(|e| warn!(device = %device.name, error = %e, "dante: product info not read"))
        .ok();
    let board = info_client::board_info(our, ip, device.device_id, INFO_WAIT)
        .inspect_err(|e| warn!(device = %device.name, error = %e, "dante: board info not read"))
        .ok();
    let encoding = read_directory(ex)
        .inspect_err(|e| warn!(device = %device.name, error = %e, "dante: encoding not read"))
        .ok()
        .and_then(|d| d.encoding());
    let info = info_view(product.as_ref(), board.as_ref(), encoding);
    info!(device = %device.name, info = ?info, "dante: device info read");
    info
}

/// The settings as the API shows them (latencies in µs).
pub fn settings_view(s: &control::Settings) -> api::Settings {
    let us = |ns: Option<u32>| ns.map(|n| n / 1000);
    api::Settings {
        sample_rate: s.sample_rate,
        pullup: s.pullup,
        latency_configured_us: us(s.latency_configured_ns),
        latency_active_us: us(s.latency_active_ns),
        latency_default_us: us(s.latency_default_ns),
        latency_min_us: us(s.latency_min_ns),
        latency_max_us: us(s.latency_max_ns),
    }
}

/// A device's channels and settings. A settings read that fails is logged
/// and leaves `settings` `None`; the channels read decides the result.
pub fn read_device(c: &mut impl Call, name: &str) -> Result<Read, ReadError> {
    let channels = read_channels(c)?;
    let settings = match control::settings(c) {
        Ok(s) => Some(settings_view(&s)),
        Err(e) => {
            warn!(device = %name, error = %e, "dante: settings not read");
            None
        }
    };
    Ok(Read {
        channels,
        settings,
        info: None,
        clock: None,
        network: None,
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
            let want_info = read(state).needs_info(&device);
            let result = Udp::open(ip, device.arc, &seq, REPLY_TIMEOUT, &requests, stop)
                .map_err(|e| format!("socket: {e}"))
                .and_then(|mut udp| {
                    let mut r = read_device(&mut udp, &device.name).map_err(|e| e.to_string())?;
                    if want_info {
                        r.info = read_info(ip, &device, &mut udp);
                    }
                    r.clock = Some(read_clock(ip, &device));
                    r.network = Some(read_network(ip, &device));
                    Ok(r)
                });
            match &result {
                Ok(r) => {
                    info!(device = %device.name, tx = r.channels.tx.len(), rx = r.channels.rx.len(),
                    settings = ?r.settings,
                    clock = ?r.clock,
                    network = ?r.network,
                    ms = started.elapsed().as_millis() as u64, "dante: channels read")
                }
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

    fn read(label: &str) -> Read {
        Read {
            channels: channels(label),
            settings: None,
            info: None,
            clock: None,
            network: None,
        }
    }

    #[test]
    fn network_view_writes_addresses_and_a_read_replaces_the_last_one() {
        let ports = [
            NetInterface {
                link_mbit: 1000,
                flags: 1,
                mac: [0, 0x1d, 0xc1, 0x0a, 0x0b, 0x0c],
                ip: Ipv4Addr::new(10, 0, 0, 5),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
                gateway: Ipv4Addr::new(10, 0, 0, 1),
                dns: Ipv4Addr::UNSPECIFIED,
            },
            NetInterface {
                link_mbit: 0,
                flags: 0,
                mac: [0, 0x1d, 0xc1, 0x0a, 0x0b, 0x0d],
                ip: Ipv4Addr::UNSPECIFIED,
                netmask: Ipv4Addr::UNSPECIFIED,
                gateway: Ipv4Addr::UNSPECIFIED,
                dns: Ipv4Addr::UNSPECIFIED,
            },
        ];
        let v = network_view(&ports);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].link_mbit, 1000);
        assert_eq!(v[0].mac, "00:1d:c1:0a:0b:0c");
        assert_eq!(v[0].ip, "10.0.0.5");
        assert_eq!(v[0].netmask, "255.255.255.0");
        assert_eq!(v[0].gateway, "10.0.0.1");
        assert_eq!(v[1].link_mbit, 0);
        assert_eq!(v[1].ip, "0.0.0.0");

        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        let now = Instant::now();
        let with = |network| Read {
            network,
            ..read("Vox")
        };
        s.record(&dev("Desk", 1), Ok(with(Some(Ok(v.clone())))), now, 1);
        assert_eq!(s.view().devices[0].network, Some(v.clone()));
        s.record(&dev("Desk", 1), Ok(with(None)), now, 2);
        assert_eq!(s.view().devices[0].network, Some(v));
        s.record(
            &dev("Desk", 1),
            Ok(with(Some(Err("no reply".into())))),
            now,
            3,
        );
        let d = &s.view().devices[0];
        assert_eq!(d.network, None);
        assert_eq!(d.network_error.as_deref(), Some("no reply"));
    }

    fn status(ports: Vec<u16>, gm: [u8; 6]) -> ClockStatus {
        ClockStatus {
            clock_state: 3,
            servo_state: 3,
            stratum: 2,
            frequency_ppb: -1200,
            device: [0, 0x1d, 0xc1, 0, 0, 1],
            master: [0, 0x1d, 0xc1, 0, 0, 9],
            grandmaster: gm,
            ports,
            capabilities: 0,
            max_drift_ppb: 100_000,
            word_clock: Some(1),
        }
    }

    #[test]
    fn clock_view_names_roles_and_states_and_keeps_unknown_codes() {
        let f = clock_view(&status(vec![9, 3], [0, 0x1d, 0xc1, 0, 0, 9]));
        assert!(!f.leader);
        assert!(!f.grandmaster);
        assert_eq!(f.state, "disciplined");
        assert_eq!(f.sync, "synchronized");
        assert_eq!(f.ports, vec!["follower".to_owned(), "disabled".to_owned()]);
        assert_eq!(f.id, "00:1d:c1:00:00:01");
        assert_eq!(f.master, "00:1d:c1:00:00:09");
        assert_eq!(f.frequency_ppb, -1200);
        assert_eq!(f.word_clock.as_deref(), Some("none"));
        let mut c = status(vec![6, 4], [0, 0x1d, 0xc1, 0, 0, 1]);
        c.clock_state = 2;
        c.servo_state = 5;
        c.word_clock = None;
        let l = clock_view(&c);
        assert!(l.leader);
        assert!(l.grandmaster);
        assert_eq!(l.state, "undisciplined");
        assert_eq!(l.sync, "servo 5");
        assert_eq!(l.ports, vec!["leader".to_owned(), "state 4".to_owned()]);
        assert_eq!(l.word_clock, None);
    }

    #[test]
    fn a_clock_read_replaces_the_last_one_and_a_failure_shows_why() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        let now = Instant::now();
        let c = clock_view(&status(vec![9], [0; 6]));
        let with = |clock| Read {
            clock,
            ..read("Vox")
        };
        s.record(&dev("Desk", 1), Ok(with(Some(Ok(c.clone())))), now, 1);
        assert_eq!(s.view().devices[0].clock, Some(c.clone()));
        // A read that did not ask (a change's read-back) keeps it.
        s.record(&dev("Desk", 1), Ok(with(None)), now, 2);
        assert_eq!(s.view().devices[0].clock, Some(c));
        s.record(
            &dev("Desk", 1),
            Ok(with(Some(Err("no reply".into())))),
            now,
            3,
        );
        let v = &s.view().devices[0];
        assert_eq!(v.clock, None);
        assert_eq!(v.clock_error.as_deref(), Some("no reply"));
    }

    fn version(major: u8, minor: u8, patch: u16, build: Option<u32>) -> info_client::Version {
        info_client::Version {
            major,
            minor,
            patch,
            build,
        }
    }

    #[test]
    fn info_view_takes_product_board_and_encoding() {
        let product = Product {
            manufacturer_id: [0; 8],
            product_id: [0; 8],
            manufacturer: "Yamaha".into(),
            model: "Dante-MY16-AUD2".into(),
            software: Some(version(1, 2, 3, None)),
            firmware: Some(version(4, 2, 5, Some(1))),
            version: Some(version(2, 0, 0, None)),
        };
        let board = Board {
            board: "Bklyn2".into(),
            model: "Brooklyn II".into(),
            software: Some(version(4, 1, 6, Some(7))),
            hardware: Some(version(1, 0, 0, None)),
            api: None,
            boot: None,
        };
        let i = info_view(Some(&product), Some(&board), Some(24)).unwrap();
        assert_eq!(i.manufacturer.as_deref(), Some("Yamaha"));
        assert_eq!(i.model.as_deref(), Some("Dante-MY16-AUD2"));
        assert_eq!(i.software_version.as_deref(), Some("1.2.3"));
        assert_eq!(i.firmware_version.as_deref(), Some("4.2.5.1"));
        assert_eq!(i.product_version.as_deref(), Some("2.0.0"));
        assert_eq!(i.platform.as_deref(), Some("Brooklyn II"));
        assert_eq!(i.platform_software.as_deref(), Some("4.1.6.7"));
        assert_eq!(i.platform_hardware.as_deref(), Some("1.0.0"));
        assert_eq!(i.encoding_bits, Some(24));
        // An empty platform model falls back to the board's short name; an
        // empty text is absent.
        let short = Board {
            model: String::new(),
            ..board
        };
        let blank = Product {
            manufacturer: String::new(),
            ..product
        };
        let i = info_view(Some(&blank), Some(&short), None).unwrap();
        assert_eq!(i.platform.as_deref(), Some("Bklyn2"));
        assert_eq!(i.manufacturer, None);
        assert_eq!(i.encoding_bits, None);
        assert_eq!(info_view(None, None, None), None);
        assert_eq!(
            info_view(None, None, Some(32)).and_then(|i| i.encoding_bits),
            Some(32)
        );
    }

    #[test]
    fn info_is_read_once_kept_and_read_again_when_the_records_change() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        assert!(s.needs_info(&dev("Desk", 1)));
        let info = api::Info {
            manufacturer: Some("Yamaha".into()),
            model: Some("RIO".into()),
            encoding_bits: Some(24),
            ..api::Info::default()
        };
        let now = Instant::now();
        s.record(
            &dev("Desk", 1),
            Ok(Read {
                info: Some(info.clone()),
                ..read("Vox")
            }),
            now,
            1,
        );
        assert!(!s.needs_info(&dev("Desk", 1)));
        let v = &s.view().devices[0];
        assert_eq!(v.info, Some(info.clone()));
        // The device's own word wins over mDNS.
        assert_eq!(v.manufacturer.as_deref(), Some("Yamaha"));
        assert_eq!(v.model.as_deref(), Some("RIO"));
        s.record(&dev("Desk", 1), Ok(read("Vox")), now + REFRESH, 2);
        assert_eq!(s.view().devices[0].info, Some(info));
        s.apply(Change::Changed(dev("Desk", 1)));
        assert!(s.needs_info(&dev("Desk", 1)));
        assert!(!s.needs_info(&dev("Nobody", 9)));
    }

    #[test]
    fn settings_are_shown_in_us_and_kept_when_a_read_has_none() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        let set = control::Settings {
            sample_rate: Some(48000),
            pullup: Some(0),
            latency_configured_ns: Some(1_000_000),
            latency_active_ns: Some(1_000_000),
            latency_default_ns: Some(1_000_000),
            latency_min_ns: Some(150_000),
            latency_max_ns: Some(5_000_000),
            all: BTreeMap::new(),
        };
        let now = Instant::now();
        s.record(
            &dev("Desk", 1),
            Ok(Read {
                channels: channels("Vox"),
                settings: Some(settings_view(&set)),
                info: None,
                clock: None,
                network: None,
            }),
            now,
            1,
        );
        let v = s.view().devices[0].settings.clone().unwrap();
        assert_eq!(v.sample_rate, Some(48000));
        assert_eq!(v.latency_active_us, Some(1000));
        assert_eq!(v.latency_min_us, Some(150));
        assert_eq!(v.latency_max_us, Some(5000));
        s.record(&dev("Desk", 1), Ok(read("Vox")), now + REFRESH, 2);
        assert_eq!(s.view().devices[0].settings, Some(v));
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
        s.record(&dev("Desk", 1), Ok(read("Vox")), now, 42);
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
        s.record(&dev("Desk", 1), Ok(read("Vox")), now, 42);
        s.record(&dev("Desk", 1), Err("no reply".into()), now + REFRESH, 99);
        let v = s.view();
        assert_eq!(v.devices[0].channels, Some(channels("Vox")));
        assert_eq!(v.devices[0].read_at, Some(42));
        assert_eq!(v.devices[0].error.as_deref(), Some("no reply"));
        s.record(&dev("Desk", 1), Ok(read("Bass")), now + REFRESH * 2, 100);
        assert_eq!(s.view().devices[0].error, None);
    }

    #[test]
    fn changed_records_make_it_due_and_drop_a_read_of_the_old_records() {
        let mut s = State::on("eth1".into(), Ipv4Addr::new(10, 0, 0, 100));
        s.apply(Change::Added(dev("Desk", 1)));
        let now = Instant::now();
        s.record(&dev("Desk", 1), Ok(read("Vox")), now, 42);
        s.apply(Change::Changed(dev("Desk", 2)));
        assert_eq!(s.due(now), vec![dev("Desk", 2)]);
        // A read that started before the change lands after it: ignored.
        s.record(&dev("Desk", 1), Ok(read("Old")), now, 43);
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
        s.record(&dev("Desk", 1), Ok(read("Vox")), Instant::now(), 1);
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
