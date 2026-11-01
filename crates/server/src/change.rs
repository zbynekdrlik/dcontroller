//! The writes to a Dante device: subscriptions, renames and settings
//! (latency, sample rate, encoding). Each one is asked for by a user who
//! confirmed it on the page, runs one at a time, is read back from the
//! device and logged (who, when, what, the value before, asked for and
//! after) to the journal and to the changes log the page lists.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dante_stack::arc::client::{REPLY_TIMEOUT, Udp, read_directory};
use dante_stack::arc::control::{self, WriteError};
use dante_stack::ctl::Requests;
use dante_stack::device::{valid_channel_name, valid_device_name};
use dante_stack::info::client as info_client;
use dcontroller_api as api;
use tracing::{info, warn};

use crate::dante::{self, Shared};

/// How many entries `GET /api/changes` lists.
pub const LIST: usize = 200;

/// The changes log (JSON lines) and the lock that runs changes one at a time.
#[derive(Debug)]
pub struct Changes {
    path: PathBuf,
    lock: Mutex<()>,
}

fn same(a: &Option<api::Source>, b: &Option<api::Source>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.channel.eq_ignore_ascii_case(&b.channel) && a.device.eq_ignore_ascii_case(&b.device)
        }
        _ => false,
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Checks a change before anything is sent: channel from 1, names Dante accepts.
pub fn validate(req: &api::SubscriptionChange) -> Result<(), String> {
    if req.rx_channel == 0 {
        return Err("RX channel numbers start at 1".into());
    }
    if let Some(s) = &req.source {
        if !valid_channel_name(&s.channel) {
            return Err(format!("not a Dante channel name: {:?}", s.channel));
        }
        if !valid_device_name(&s.device) {
            return Err(format!("not a Dante device name: {:?}", s.device));
        }
    }
    Ok(())
}

impl Changes {
    pub fn new(path: PathBuf) -> Self {
        Changes {
            path,
            lock: Mutex::new(()),
        }
    }

    /// The last [`LIST`] entries, newest first. A line that does not parse is
    /// skipped and logged.
    pub fn list(&self) -> Result<Vec<api::ChangeEntry>, String> {
        let f = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("read {}: {e}", self.path.display())),
        };
        let mut out = Vec::new();
        for (n, line) in BufReader::new(f).lines().enumerate() {
            let line = line.map_err(|e| format!("read {}: {e}", self.path.display()))?;
            match serde_json::from_str::<api::ChangeEntry>(&line) {
                Ok(e) => out.push(e),
                Err(e) => {
                    warn!(path = %self.path.display(), line = n + 1, error = %e, "changes log: line skipped")
                }
            }
        }
        out.reverse();
        out.truncate(LIST);
        Ok(out)
    }

    fn append(&self, e: &api::ChangeEntry) -> Result<(), String> {
        let line = serde_json::to_string(e).map_err(|e| e.to_string())?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("open {}: {e}", self.path.display()))?;
        writeln!(f, "{line}").map_err(|e| format!("write {}: {e}", self.path.display()))
    }

    /// Runs one confirmed change by `user`: reads the channel, sends the
    /// subscription or its removal, reads it back, updates the shown state
    /// and logs it. Blocking (network); one change at a time.
    pub fn apply(
        &self,
        dante: &Shared,
        user: &str,
        req: &api::SubscriptionChange,
    ) -> api::ChangeResult {
        let _one = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        info!(%user, rx_device = %req.rx_device, rx_channel = req.rx_channel,
            source = ?req.source, "change: asked");
        if let Err(message) = validate(req) {
            warn!(%user, %message, "change: refused before sending");
            return api::ChangeResult {
                ok: false,
                message,
                channel: None,
            };
        }
        let target = dante::State::target(
            &dante.read().unwrap_or_else(|e| e.into_inner()),
            &req.rx_device,
        );
        let Some((ip, device)) = target else {
            let message = format!(
                "{} is not listed (discovery off or device gone)",
                req.rx_device
            );
            warn!(%user, %message, "change: refused before sending");
            return api::ChangeResult {
                ok: false,
                message,
                channel: None,
            };
        };
        let seq = AtomicU16::new(1);
        let requests = Requests::default();
        let stop = AtomicBool::new(false);
        let find =
            |rx: &[control::RxChannel]| rx.iter().find(|r| r.channel == req.rx_channel).cloned();
        let source_of = |r: &control::RxChannel| match (&r.tx_channel, &r.tx_device) {
            (Some(c), Some(d)) => Some(api::Source {
                channel: c.clone(),
                device: d.clone(),
            }),
            _ => None,
        };
        let run =
            || -> Result<(control::RxChannel, Vec<control::RxChannel>, Option<String>), String> {
                let mut udp = Udp::open(ip, device.arc, &seq, REPLY_TIMEOUT, &requests, &stop)
                    .map_err(|e| format!("socket: {e}"))?;
                let before =
                    control::rx_channels(&mut udp).map_err(|e| format!("read before: {e}"))?;
                let ch = find(&before).ok_or_else(|| {
                    format!("{} has no RX channel {}", device.name, req.rx_channel)
                })?;
                let sent = match &req.source {
                    Some(s) => control::subscribe(&mut udp, req.rx_channel, &s.channel, &s.device),
                    None => control::unsubscribe(&mut udp, req.rx_channel),
                };
                let send_error = sent.err().map(|e| format!("device refused: {e}"));
                let after =
                    control::rx_channels(&mut udp).map_err(|e| format!("read back: {e}"))?;
                Ok((ch, after, send_error))
            };
        let (entry, result) = match run() {
            Err(message) => {
                warn!(%user, device = %device.name, rx_channel = req.rx_channel, %message, "change: failed");
                let entry = api::ChangeEntry {
                    at: now(),
                    user: user.to_owned(),
                    kind: "subscription".into(),
                    rx_device: device.name.clone(),
                    rx_channel: req.rx_channel,
                    rx_label: String::new(),
                    before: None,
                    requested: req.source.clone(),
                    after: None,
                    ok: false,
                    message: message.clone(),
                    ..api::ChangeEntry::default()
                };
                (
                    entry,
                    api::ChangeResult {
                        ok: false,
                        message,
                        channel: None,
                    },
                )
            }
            Ok((before, after, send_error)) => {
                let now_ch = find(&after);
                let after_src = now_ch.as_ref().and_then(source_of);
                let ok = send_error.is_none() && now_ch.is_some() && same(&after_src, &req.source);
                let message = match (&send_error, ok) {
                    (Some(e), _) => e.clone(),
                    (None, true) => "done, read back from the device".into(),
                    (None, false) => format!(
                        "the device answered OK but reports {} now",
                        after_src
                            .as_ref()
                            .map_or("no subscription".into(), |s| format!(
                                "{} @ {}",
                                s.channel, s.device
                            ))
                    ),
                };
                let shown = dante::rx_view(&after);
                let channel = shown.iter().find(|r| r.channel == req.rx_channel).cloned();
                dante
                    .write()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_rx(&device, shown);
                let entry = api::ChangeEntry {
                    at: now(),
                    user: user.to_owned(),
                    kind: "subscription".into(),
                    rx_device: device.name.clone(),
                    rx_channel: req.rx_channel,
                    rx_label: before.label.clone(),
                    before: source_of(&before),
                    requested: req.source.clone(),
                    after: after_src,
                    ok,
                    message: message.clone(),
                    ..api::ChangeEntry::default()
                };
                (
                    entry,
                    api::ChangeResult {
                        ok,
                        message,
                        channel,
                    },
                )
            }
        };
        info!(%user, rx_device = %entry.rx_device, rx_channel = entry.rx_channel, rx_label = %entry.rx_label,
            before = ?entry.before, requested = ?entry.requested, after = ?entry.after,
            ok = entry.ok, message = %entry.message, "change: done");
        if let Err(e) = self.append(&entry) {
            warn!(error = %e, "changes log: entry not written (it is in the journal above)");
        }
        result
    }
}

/// A rename's reads: the name before, after, and the device's refusal if any.
type NamesRead = (Option<String>, Option<String>, Option<String>);

/// Checks a rename before anything is sent: channel from 1, a name Dante accepts.
pub fn validate_rename(req: &api::Rename) -> Result<(), String> {
    match (req.target, &req.name) {
        (api::RenameTarget::Tx(0) | api::RenameTarget::Rx(0), _) => {
            Err("channel numbers start at 1".into())
        }
        (api::RenameTarget::Device, None) => Err("a device needs a name".into()),
        (api::RenameTarget::Device, Some(n)) if !valid_device_name(n) => Err(format!(
            "not a Dante device name: {n:?} (1 to 31 of A-Z a-z 0-9 and -, no - at either end)"
        )),
        (_, Some(n))
            if !matches!(req.target, api::RenameTarget::Device) && !valid_channel_name(n) =>
        {
            Err(format!(
                "not a Dante channel name: {n:?} (1 to 31 characters, none of = . @)"
            ))
        }
        _ => Ok(()),
    }
}

impl Changes {
    /// Runs one confirmed rename by `user`: reads the name, sends the new
    /// one, reads it back, logs it and makes the device due for a fresh read.
    pub fn rename(&self, dante: &Shared, user: &str, req: &api::Rename) -> api::ChangeResult {
        let _one = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        info!(%user, device = %req.device, target = ?req.target, name = ?req.name, "rename: asked");
        let refuse = |message: String| {
            warn!(%user, %message, "rename: refused before sending");
            api::ChangeResult {
                ok: false,
                message,
                channel: None,
            }
        };
        if let Err(m) = validate_rename(req) {
            return refuse(m);
        }
        let target = dante::State::target(
            &dante.read().unwrap_or_else(|e| e.into_inner()),
            &req.device,
        );
        let Some((ip, device)) = target else {
            return refuse(format!(
                "{} is not listed (discovery off or device gone)",
                req.device
            ));
        };
        let seq = AtomicU16::new(1);
        let requests = Requests::default();
        let stop = AtomicBool::new(false);
        let (what, channel) = match req.target {
            api::RenameTarget::Tx(c) => ("tx", c),
            api::RenameTarget::Rx(c) => ("rx", c),
            api::RenameTarget::Device => ("device", 0),
        };
        let read = |udp: &mut Udp<'_>| -> Result<Option<String>, String> {
            match req.target {
                api::RenameTarget::Device => control::device_name(udp)
                    .map(Some)
                    .map_err(|e| e.to_string()),
                api::RenameTarget::Tx(c) => control::labels(udp)
                    .map(|l| l.get(control::Dir::Tx, c).map(str::to_owned))
                    .map_err(|e| e.to_string()),
                api::RenameTarget::Rx(c) => control::labels(udp)
                    .map(|l| l.get(control::Dir::Rx, c).map(str::to_owned))
                    .map_err(|e| e.to_string()),
            }
        };
        let run = || -> Result<NamesRead, String> {
            let mut udp = Udp::open(ip, device.arc, &seq, REPLY_TIMEOUT, &requests, &stop)
                .map_err(|e| format!("socket: {e}"))?;
            let before = read(&mut udp).map_err(|e| format!("read before: {e}"))?;
            if before.is_none() {
                return Err(format!("{} has no {what} channel {channel}", device.name));
            }
            let sent = match req.target {
                api::RenameTarget::Device => {
                    control::rename_device(&mut udp, req.name.as_deref().unwrap_or_default())
                }
                api::RenameTarget::Tx(c) => {
                    control::rename_channel(&mut udp, control::Dir::Tx, c, req.name.as_deref())
                }
                api::RenameTarget::Rx(c) => {
                    control::rename_channel(&mut udp, control::Dir::Rx, c, req.name.as_deref())
                }
            };
            let send_error = sent.err().map(|e| format!("device refused: {e}"));
            let after = read(&mut udp).map_err(|e| format!("read back: {e}"))?;
            Ok((before, after, send_error))
        };
        let mut entry = api::ChangeEntry {
            at: now(),
            user: user.to_owned(),
            kind: "rename".into(),
            what: what.into(),
            rx_device: device.name.clone(),
            rx_channel: channel,
            name_asked: req.name.clone(),
            ..api::ChangeEntry::default()
        };
        let result = match run() {
            Err(message) => api::ChangeResult {
                ok: false,
                message,
                channel: None,
            },
            Ok((before, after, send_error)) => {
                entry.rx_label = before.clone().unwrap_or_default();
                entry.name_before = before;
                entry.name_after = after.clone();
                // A reset asks for the default, which only the device knows: done when it took it.
                let ok = send_error.is_none()
                    && match &req.name {
                        Some(n) => after.as_deref() == Some(n.as_str()),
                        None => after.is_some(),
                    };
                let message = match (&send_error, ok) {
                    (Some(e), _) => e.clone(),
                    (None, true) => format!(
                        "done, read back from the device: {}",
                        after.as_deref().unwrap_or_default()
                    ),
                    (None, false) => format!(
                        "the device answered OK but reports {:?} now",
                        after.as_deref().unwrap_or_default()
                    ),
                };
                api::ChangeResult {
                    ok,
                    message,
                    channel: None,
                }
            }
        };
        entry.ok = result.ok;
        entry.message = result.message.clone();
        // Show the new names at once: a fresh read of the device's channels.
        let fresh = Udp::open(ip, device.arc, &seq, REPLY_TIMEOUT, &requests, &stop)
            .map_err(|e| format!("socket: {e}"))
            .and_then(|mut udp| {
                dante::read_device(&mut udp, &device.name).map_err(|e| e.to_string())
            });
        {
            let mut st = dante.write().unwrap_or_else(|e| e.into_inner());
            match fresh {
                Ok(ch) => st.record(&device, Ok(ch), std::time::Instant::now(), now()),
                Err(e) => {
                    warn!(device = %device.name, error = %e, "rename: fresh read failed, read again soon");
                    st.mark_due(&device);
                }
            }
        }
        info!(%user, device = %entry.rx_device, what = %entry.what, channel = entry.rx_channel,
            before = ?entry.name_before, asked = ?entry.name_asked, after = ?entry.name_after,
            ok = entry.ok, message = %entry.message, "rename: done");
        if let Err(e) = self.append(&entry) {
            warn!(error = %e, "changes log: entry not written (it is in the journal above)");
        }
        result
    }
}

/// How long a sample-rate or encoding write waits to read its value back.
const WRITE_WAIT: Duration = Duration::from_secs(5);
/// How often an encoding write reads the device's channels again.
const ENCODING_POLL: Duration = Duration::from_millis(500);
/// The encodings Dante devices offer.
pub const ENCODINGS: [u32; 3] = [16, 24, 32];

/// A setting's value as the log and the page write it.
pub fn setting_text(s: api::Setting) -> String {
    match s {
        api::Setting::LatencyUs(us) => format!("{:.2} ms", f64::from(us) / 1000.0),
        api::Setting::SampleRateHz(hz) => format!("{hz} Hz"),
        api::Setting::EncodingBits(b) => format!("{b} bit"),
    }
}

/// Checks a setting before anything is sent, against what the device last
/// reported: a latency within its range, a rate it lists, a Dante encoding.
pub fn validate_setting(
    s: api::Setting,
    settings: Option<&api::Settings>,
    info: Option<&api::Info>,
) -> Result<(), String> {
    match s {
        api::Setting::LatencyUs(us) => {
            let range = settings.and_then(|s| s.latency_min_us.zip(s.latency_max_us));
            match range {
                Some((lo, hi)) if (lo..=hi).contains(&us) => Ok(()),
                Some((lo, hi)) => Err(format!(
                    "{} is outside the device's latency range {} to {}",
                    setting_text(s),
                    setting_text(api::Setting::LatencyUs(lo)),
                    setting_text(api::Setting::LatencyUs(hi))
                )),
                None => Err("the device reports no latency range".into()),
            }
        }
        api::Setting::SampleRateHz(hz) => {
            let rates = info.map(|i| i.sample_rates.as_slice()).unwrap_or_default();
            if rates.contains(&hz) {
                Ok(())
            } else if rates.is_empty() {
                Err("the device lists no sample rates".into())
            } else {
                Err(format!(
                    "{hz} Hz is not one of the device's rates {rates:?}"
                ))
            }
        }
        api::Setting::EncodingBits(b) if ENCODINGS.contains(&b) => Ok(()),
        api::Setting::EncodingBits(b) => Err(format!("{b} bit is not a Dante encoding")),
    }
}

/// The message for a latency write's outcome: ok and text.
pub fn latency_outcome(asked_us: u32, r: &Result<control::Settings, WriteError>) -> (bool, String) {
    let ms = |ns: u32| setting_text(api::Setting::LatencyUs(ns / 1000));
    match r {
        Ok(after) => (
            true,
            format!(
                "done, read back from the device: latency {}",
                after.latency_configured_ns.map_or("?".into(), ms)
            ),
        ),
        Err(WriteError::OthersChanged(keys)) => (
            false,
            format!(
                "WARNING: other settings changed with the latency, check the device: {}",
                keys.iter()
                    .map(|(k, b, a)| format!("key 0x{k:04x} {b:?} -> {a:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        Err(WriteError::NotConfirmed { read, .. }) => (
            false,
            format!(
                "the device answered but reports latency {} (asked {})",
                read.map_or("none".into(), ms),
                setting_text(api::Setting::LatencyUs(asked_us))
            ),
        ),
        Err(WriteError::Read(e)) => (false, format!("not confirmed: {e}")),
    }
}

/// The message for a sample-rate write's outcome: ok and text.
pub fn rate_outcome(
    asked: u32,
    r: &Result<info_client::SampleRate, info_client::InfoError>,
) -> (bool, String) {
    match r {
        Ok(s) if s.current == asked => (true, format!("done, the device runs at {asked} Hz")),
        Ok(s) if s.pending == Some(asked) => (
            true,
            format!(
                "set to {asked} Hz, applies at the device's next start (runs at {} Hz now)",
                s.current
            ),
        ),
        Ok(s) => (false, format!("the device reports {} Hz", s.current)),
        Err(e) => (false, format!("not confirmed: {e}")),
    }
}

impl Changes {
    /// Runs one confirmed setting by `user`: checks it against the device's
    /// last read, reads the value before, writes it, reads it back, logs it
    /// and makes the device due for a fresh read (its info included).
    pub fn setting(
        &self,
        dante: &Shared,
        user: &str,
        req: &api::SettingChange,
    ) -> api::ChangeResult {
        let _one = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        info!(%user, device = %req.device, setting = ?req.setting, "setting: asked");
        let refuse = |message: String| {
            warn!(%user, %message, "setting: refused before sending");
            api::ChangeResult {
                ok: false,
                message,
                channel: None,
            }
        };
        let (target, (settings, info)) = {
            let st = dante.read().unwrap_or_else(|e| e.into_inner());
            (st.target(&req.device), st.known(&req.device))
        };
        let Some((ip, device)) = target else {
            return refuse(format!(
                "{} is not listed (discovery off or device gone)",
                req.device
            ));
        };
        if let Err(m) = validate_setting(req.setting, settings.as_ref(), info.as_ref()) {
            return refuse(m);
        }
        let seq = AtomicU16::new(1);
        let requests = Requests::default();
        let stop = AtomicBool::new(false);
        let dev_ip = *device.arc.ip();
        let open = || {
            Udp::open(ip, device.arc, &seq, REPLY_TIMEOUT, &requests, &stop)
                .map_err(|e| format!("socket: {e}"))
        };
        // (before, after, ok, message); before/after as text.
        let run = || -> Result<(Option<String>, Option<String>, bool, String), String> {
            match req.setting {
                api::Setting::LatencyUs(us) => {
                    let mut udp = open()?;
                    let ms = |s: &control::Settings| {
                        s.latency_configured_ns
                            .map(|ns| setting_text(api::Setting::LatencyUs(ns / 1000)))
                    };
                    let before =
                        control::settings(&mut udp).map_err(|e| format!("read before: {e}"))?;
                    let r = control::set_latency(&mut udp, us.saturating_mul(1000));
                    let after = match &r {
                        Ok(a) => ms(a),
                        Err(_) => control::settings(&mut udp).ok().as_ref().and_then(ms),
                    };
                    let (ok, message) = latency_outcome(us, &r);
                    Ok((ms(&before), after, ok, message))
                }
                api::Setting::SampleRateHz(hz) => {
                    let text = |s: &info_client::SampleRate| match s.pending {
                        Some(p) => format!("{} Hz (pending {p} Hz)", s.current),
                        None => format!("{} Hz", s.current),
                    };
                    let before = info_client::sample_rate(ip, dev_ip, device.device_id, WRITE_WAIT)
                        .map_err(|e| format!("read before: {e}"))?;
                    let r =
                        info_client::set_sample_rate(ip, dev_ip, device.device_id, hz, WRITE_WAIT);
                    let after = match &r {
                        Ok(a) => Some(text(a)),
                        Err(_) => {
                            info_client::sample_rate(ip, dev_ip, device.device_id, WRITE_WAIT)
                                .ok()
                                .as_ref()
                                .map(text)
                        }
                    };
                    let (ok, message) = rate_outcome(hz, &r);
                    Ok((Some(text(&before)), after, ok, message))
                }
                api::Setting::EncodingBits(bits) => {
                    let mut udp = open()?;
                    let enc = |udp: &mut Udp<'_>| {
                        read_directory(udp)
                            .map(|d| d.encoding())
                            .map_err(|e| e.to_string())
                    };
                    let text =
                        |b: Option<u32>| b.map(|b| setting_text(api::Setting::EncodingBits(b)));
                    let before = enc(&mut udp).map_err(|e| format!("read before: {e}"))?;
                    info_client::set_encoding(ip, dev_ip, bits)
                        .map_err(|e| format!("not sent: {e}"))?;
                    let end = Instant::now() + WRITE_WAIT;
                    let mut now_bits = before;
                    while Instant::now() < end {
                        std::thread::sleep(ENCODING_POLL);
                        if let Ok(b) = enc(&mut udp) {
                            now_bits = b;
                            if b == Some(bits) {
                                break;
                            }
                        }
                    }
                    let ok = now_bits == Some(bits);
                    let message = if ok {
                        format!("done, read back from the device: {bits} bit")
                    } else {
                        format!(
                            "not confirmed: the device's channels read {} after {} s",
                            text(now_bits).unwrap_or_else(|| "no encoding".into()),
                            WRITE_WAIT.as_secs()
                        )
                    };
                    Ok((text(before), text(now_bits), ok, message))
                }
            }
        };
        let mut entry = api::ChangeEntry {
            at: now(),
            user: user.to_owned(),
            kind: "setting".into(),
            what: req.setting.what().into(),
            rx_device: device.name.clone(),
            name_asked: Some(setting_text(req.setting)),
            ..api::ChangeEntry::default()
        };
        let result = match run() {
            Err(message) => api::ChangeResult {
                ok: false,
                message,
                channel: None,
            },
            Ok((before, after, ok, message)) => {
                entry.name_before = before;
                entry.name_after = after;
                api::ChangeResult {
                    ok,
                    message,
                    channel: None,
                }
            }
        };
        entry.ok = result.ok;
        entry.message = result.message.clone();
        // Show the device as it is now: its info read again, its settings at once.
        let fresh = open().and_then(|mut udp| {
            dante::read_device(&mut udp, &device.name).map_err(|e| e.to_string())
        });
        {
            let mut st = dante.write().unwrap_or_else(|e| e.into_inner());
            st.forget_info(&device);
            match fresh {
                Ok(r) => st.record(&device, Ok(r), Instant::now(), now()),
                Err(e) => {
                    warn!(device = %device.name, error = %e, "setting: fresh read failed, read again soon");
                    st.mark_due(&device);
                }
            }
        }
        info!(%user, device = %entry.rx_device, what = %entry.what,
            before = ?entry.name_before, asked = ?entry.name_asked, after = ?entry.name_after,
            ok = entry.ok, message = %entry.message, "setting: done");
        if let Err(e) = self.append(&entry) {
            warn!(error = %e, "changes log: entry not written (it is in the journal above)");
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_checked_against_what_the_device_reports() {
        let set = api::Settings {
            latency_min_us: Some(150),
            latency_max_us: Some(5000),
            ..api::Settings::default()
        };
        let info = api::Info {
            sample_rates: vec![44100, 48000, 96000],
            ..api::Info::default()
        };
        let v = |s| validate_setting(s, Some(&set), Some(&info));
        assert!(v(api::Setting::LatencyUs(150)).is_ok());
        assert!(v(api::Setting::LatencyUs(5000)).is_ok());
        assert!(
            v(api::Setting::LatencyUs(100))
                .unwrap_err()
                .contains("outside the device's latency range 0.15 ms to 5.00 ms")
        );
        assert!(v(api::Setting::LatencyUs(6000)).is_err());
        assert!(
            validate_setting(api::Setting::LatencyUs(1000), None, None)
                .unwrap_err()
                .contains("no latency range")
        );
        assert!(v(api::Setting::SampleRateHz(48000)).is_ok());
        assert!(
            v(api::Setting::SampleRateHz(88200))
                .unwrap_err()
                .contains("not one of")
        );
        assert!(
            validate_setting(api::Setting::SampleRateHz(48000), None, None)
                .unwrap_err()
                .contains("no sample rates")
        );
        assert!(v(api::Setting::EncodingBits(24)).is_ok());
        assert!(
            v(api::Setting::EncodingBits(20))
                .unwrap_err()
                .contains("not a Dante encoding")
        );
        assert_eq!(setting_text(api::Setting::LatencyUs(250)), "0.25 ms");
        assert_eq!(setting_text(api::Setting::SampleRateHz(96000)), "96000 Hz");
        assert_eq!(setting_text(api::Setting::EncodingBits(32)), "32 bit");
    }

    #[test]
    fn write_outcomes_read_plainly_and_others_changed_warns() {
        let after = control::Settings {
            latency_configured_ns: Some(500_000),
            ..control::Settings::default()
        };
        assert_eq!(
            latency_outcome(500, &Ok(after)),
            (
                true,
                "done, read back from the device: latency 0.50 ms".to_owned()
            )
        );
        let (ok, m) = latency_outcome(
            500,
            &Err(WriteError::OthersChanged(vec![(0x2808, Some(0), Some(1))])),
        );
        assert!(!ok);
        assert!(m.starts_with("WARNING: other settings changed"));
        assert!(m.contains("key 0x2808 Some(0) -> Some(1)"));
        assert_eq!(
            latency_outcome(
                500,
                &Err(WriteError::NotConfirmed {
                    asked: 500_000,
                    read: Some(1_000_000)
                })
            ),
            (
                false,
                "the device answered but reports latency 1.00 ms (asked 0.50 ms)".to_owned()
            )
        );
        let rate = |current, pending| info_client::SampleRate {
            current,
            pending,
            supported: vec![48000, 96000],
        };
        assert_eq!(
            rate_outcome(96000, &Ok(rate(96000, None))),
            (true, "done, the device runs at 96000 Hz".to_owned())
        );
        assert_eq!(
            rate_outcome(96000, &Ok(rate(48000, Some(96000)))),
            (
                true,
                "set to 96000 Hz, applies at the device's next start (runs at 48000 Hz now)"
                    .to_owned()
            )
        );
        assert!(!rate_outcome(96000, &Ok(rate(48000, None))).0);
        let (ok, m) = rate_outcome(
            96000,
            &Err(info_client::InfoError::NotConfirmed {
                ask: info_client::Ask::SetSampleRate(96000),
            }),
        );
        assert!(!ok);
        assert!(m.starts_with("not confirmed"));
    }

    fn req(ch: u16, src: Option<(&str, &str)>) -> api::SubscriptionChange {
        api::SubscriptionChange {
            rx_device: "stage".into(),
            rx_channel: ch,
            source: src.map(|(c, d)| api::Source {
                channel: c.into(),
                device: d.into(),
            }),
        }
    }

    #[test]
    fn validate_refuses_channel_zero_and_names_dante_refuses() {
        assert!(validate(&req(1, Some(("Mix L", "desk")))).is_ok());
        assert!(validate(&req(1, None)).is_ok());
        assert!(validate(&req(0, None)).unwrap_err().contains("start at 1"));
        assert!(
            validate(&req(1, Some(("Mix@L", "desk"))))
                .unwrap_err()
                .contains("channel name")
        );
        assert!(
            validate(&req(1, Some(("Mix L", "-desk"))))
                .unwrap_err()
                .contains("device name")
        );
        assert!(validate(&req(1, Some(("", "desk")))).is_err());
    }

    #[test]
    fn validate_rename_checks_targets_and_names() {
        let r = |target, name: Option<&str>| api::Rename {
            device: "desk".into(),
            target,
            name: name.map(Into::into),
        };
        use api::RenameTarget::{Device, Rx, Tx};
        assert!(validate_rename(&r(Tx(1), Some("Vox 1"))).is_ok());
        assert!(validate_rename(&r(Rx(2), None)).is_ok());
        assert!(validate_rename(&r(Device, Some("stage-box"))).is_ok());
        assert!(
            validate_rename(&r(Tx(0), Some("x")))
                .unwrap_err()
                .contains("start at 1")
        );
        assert!(
            validate_rename(&r(Rx(1), Some("a.b")))
                .unwrap_err()
                .contains("channel name")
        );
        assert!(
            validate_rename(&r(Device, Some("stage box")))
                .unwrap_err()
                .contains("device name")
        );
        assert!(validate_rename(&r(Device, None)).is_err());
        // A device name is not held to the channel rule, nor a channel to the device rule.
        assert!(validate_rename(&r(Tx(1), Some("Vox (L)"))).is_ok());
    }

    #[test]
    fn same_compares_names_case_aside() {
        let s = |c: &str, d: &str| {
            Some(api::Source {
                channel: c.into(),
                device: d.into(),
            })
        };
        assert!(same(&s("Mix L", "desk"), &s("mix l", "DESK")));
        assert!(!same(&s("Mix L", "desk"), &s("Mix R", "desk")));
        assert!(same(&None, &None));
        assert!(!same(&None, &s("a", "b")));
    }

    #[test]
    fn a_change_to_an_unlisted_device_is_refused_and_logs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let changes = Changes::new(dir.path().join("changes.jsonl"));
        let off: Shared =
            std::sync::Arc::new(std::sync::RwLock::new(dante::State::off(None, None)));
        let r = changes.apply(&off, "a@b", &req(1, None));
        assert!(!r.ok);
        assert!(r.message.contains("not listed"), "{}", r.message);
        assert!(changes.list().unwrap().is_empty());
    }

    #[test]
    fn the_log_lists_newest_first_and_skips_a_bad_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("changes.jsonl");
        let changes = Changes::new(path.clone());
        assert!(changes.list().unwrap().is_empty());
        for n in 1..=3 {
            changes
                .append(&api::ChangeEntry {
                    at: n,
                    ..api::ChangeEntry::default()
                })
                .unwrap();
        }
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"not json\n")
            .unwrap();
        let at: Vec<u64> = changes.list().unwrap().iter().map(|e| e.at).collect();
        assert_eq!(at, [3, 2, 1]);
    }
}
