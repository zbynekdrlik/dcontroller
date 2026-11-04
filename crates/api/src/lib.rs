//! The JSON the server's `/api/devices` sends and the web app reads. One
//! definition for both sides, so they cannot drift apart.

use serde::{Deserialize, Serialize};

/// `GET /api/devices`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Devices {
    /// Discovery runs (a Dante interface is configured and has an IPv4).
    pub enabled: bool,
    /// The Dante interface from the config, if any.
    pub interface: Option<String>,
    /// Why discovery does not run, when it does not.
    pub error: Option<String>,
    /// The devices, sorted by name (case aside).
    pub devices: Vec<Device>,
}

/// One Dante device: what its mDNS records say, and its last ARC read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    /// Its ARC address, `ip:port`.
    pub address: String,
    /// mDNS `mf`.
    pub manufacturer: Option<String>,
    /// The model: mDNS `router_info` (board name), else `model`.
    pub model: Option<String>,
    /// The last complete read; `None` before the first one.
    pub channels: Option<Channels>,
    /// The device's settings, as last read; `None` before the first read
    /// or when the device does not answer the settings read.
    pub settings: Option<Settings>,
    /// What the device says it is (product and board info, encoding);
    /// `None` until it answers.
    pub info: Option<Info>,
    /// Its clock status at the last read; `None` before it or when it did
    /// not answer (`clock_error` says why).
    pub clock: Option<Clock>,
    pub clock_error: Option<String>,
    /// Its network interfaces at the last read, primary first; `None`
    /// before it or when it did not answer (`network_error` says why).
    pub network: Option<Vec<NetPort>>,
    pub network_error: Option<String>,
    /// Unix seconds of the last complete read.
    pub read_at: Option<u64>,
    /// The last read's error, when it failed (the previous read stays).
    pub error: Option<String>,
}

/// A device's settings (ARC 0x1100), each `None` when the device does not
/// report it. Latencies in µs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub sample_rate: Option<u32>,
    pub pullup: Option<u32>,
    pub latency_configured_us: Option<u32>,
    pub latency_active_us: Option<u32>,
    pub latency_default_us: Option<u32>,
    pub latency_min_us: Option<u32>,
    pub latency_max_us: Option<u32>,
}

/// What a device says it is: its product info and board info (UDP 8700)
/// and its encoding (from its TX channels). Each `None` when not reported.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub product_version: Option<String>,
    pub software_version: Option<String>,
    pub firmware_version: Option<String>,
    /// The Dante platform (`Brooklyn II`, `UltimoX2`).
    pub platform: Option<String>,
    pub platform_software: Option<String>,
    pub platform_hardware: Option<String>,
    /// Bits per sample, shared by all its TX channels.
    pub encoding_bits: Option<u32>,
    /// The sample rates the device takes (UDP 8700); empty when it does
    /// not say.
    #[serde(default)]
    pub sample_rates: Vec<u32>,
    /// A rate set but not yet in effect (applied at the device's next start).
    #[serde(default)]
    pub sample_rate_pending: Option<u32>,
}

/// `POST /api/setting`: one setting of one device, confirmed by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingChange {
    pub device: String,
    pub setting: Setting,
}

/// The setting to write and its new value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Setting {
    LatencyUs(u32),
    SampleRateHz(u32),
    EncodingBits(u32),
}

impl Setting {
    /// Its name in the changes log.
    pub fn what(self) -> &'static str {
        match self {
            Setting::LatencyUs(_) => "latency",
            Setting::SampleRateHz(_) => "sample_rate",
            Setting::EncodingBits(_) => "encoding",
        }
    }
}

/// A device's clock status (UDP 8700, Dante's clock stats). States the
/// stack has no name for read as their code (`state 4`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    /// Some PTP port leads.
    pub leader: bool,
    /// The device is the grandmaster.
    pub grandmaster: bool,
    /// The clock state: `disciplined`, `undisciplined`.
    pub state: String,
    /// The servo state: `synchronized`, `none`.
    pub sync: String,
    pub stratum: u8,
    pub frequency_ppb: i32,
    pub max_drift_ppb: u32,
    /// Clock ids as MACs, `aa:bb:cc:dd:ee:ff`.
    pub id: String,
    pub master: String,
    pub grandmaster_id: String,
    /// Each PTP port's state: `leader`, `follower`, `disabled`.
    pub ports: Vec<String>,
    /// `none`, `valid`, `unknown`; `None` when the device does not say.
    pub word_clock: Option<String>,
}

/// One network interface of a device (UDP 8700 network info).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetPort {
    /// Mbit/s; 0: no link.
    pub link_mbit: u16,
    pub mac: String,
    pub ip: String,
    pub netmask: String,
    pub gateway: String,
    pub dns: String,
}

/// A device's channels, read over ARC.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channels {
    pub tx: Vec<TxChannel>,
    pub rx: Vec<RxChannel>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxChannel {
    /// From 1.
    pub channel: u16,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RxChannel {
    /// From 1.
    pub channel: u16,
    pub label: String,
    /// The subscribed TX channel and device; `None` when not subscribed.
    pub source: Option<Source>,
    /// The subscription's status in Dante Controller's words
    /// (`Subscribed (unicast)`, `Unresolved`, ...), or its code in hex when
    /// unknown; `None` when not subscribed.
    pub status: Option<String>,
    /// What the status means for the audio: `connected`, `pending`, `idle`
    /// or `error`; `None` when not subscribed or the code is unknown.
    pub state: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub channel: String,
    pub device: String,
}

/// `POST /api/subscription`: set (`source` given) or remove (`None`) the
/// subscription of one RX channel. The page sends it only after the user
/// confirmed the change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionChange {
    pub rx_device: String,
    /// From 1.
    pub rx_channel: u16,
    pub source: Option<Source>,
}

/// The answer to a [`SubscriptionChange`]: the channel as the device
/// reports it after the change (read back), or why it failed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeResult {
    /// The device reports the requested subscription now.
    pub ok: bool,
    pub message: String,
    /// The channel read back from the device, when the read worked.
    pub channel: Option<RxChannel>,
}

/// `POST /api/rename`: a new label for a TX or RX channel (`name` `None`
/// resets it to the channel's default), or a new device name. Sent only
/// after the user confirmed it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rename {
    pub device: String,
    pub target: RenameTarget,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenameTarget {
    /// TX channel (from 1).
    Tx(u16),
    /// RX channel (from 1).
    Rx(u16),
    /// The device's own name.
    #[default]
    Device,
}

/// One logged change (`GET /api/changes`, newest first): a subscription
/// change, or a rename (`kind` `rename`, the names in `name_*`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChangeEntry {
    /// Unix seconds.
    pub at: u64,
    /// The Cloudflare Access e-mail, or `local` without Access.
    pub user: String,
    /// `subscription` (also when empty, as in the first log lines),
    /// `rename` or `setting`.
    pub kind: String,
    /// For a rename: `tx`, `rx` or `device`; for a setting: `latency`,
    /// `sample_rate` or `encoding`.
    pub what: String,
    /// The device the change went to (for a subscription, the receiver).
    pub rx_device: String,
    /// The channel (0 for a device rename).
    pub rx_channel: u16,
    pub rx_label: String,
    /// For a rename: the name before, asked for (`None` = default) and
    /// after; for a setting: its value before, asked for and after, as text.
    pub name_before: Option<String>,
    pub name_asked: Option<String>,
    pub name_after: Option<String>,
    /// The subscription before, as read before the change.
    pub before: Option<Source>,
    /// The subscription asked for (`None` = remove).
    pub requested: Option<Source>,
    /// The subscription read back after the change.
    pub after: Option<Source>,
    pub ok: bool,
    pub message: String,
}

/// `GET /api/meters/{device}`: the device's channel levels, as Dante
/// Controller's meters show them, from the frames it sends while someone
/// watches.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeterReport {
    pub device: String,
    /// Milliseconds since the last frame; `None` before the first.
    pub age_ms: Option<u64>,
    pub frames: u64,
    pub tx: Vec<ChannelLevel>,
    pub rx: Vec<ChannelLevel>,
    /// Why there are no levels, when there are none.
    pub error: Option<String>,
}

/// One channel's level in a meter frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChannelLevel {
    /// From 1.
    pub channel: u16,
    pub label: String,
    /// dBFS (0 when clipping); `None` muted or not sent.
    pub dbfs: Option<f32>,
    pub clip: bool,
    pub muted: bool,
}

/// `GET /api/history/{device}?from=&to=`: a receiver's packet latency per
/// RX flow over a time range, from the server's recorder, bucketed to at
/// most [`HISTORY_POINTS`] points.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub device: String,
    /// The range asked for, unix seconds.
    pub from: i64,
    pub to: i64,
    /// Seconds per bucket.
    pub step_s: i64,
    /// The stored resolution the buckets come from, seconds (1, 60, 3600).
    pub tier_s: i64,
    /// Each bucket's start, unix seconds; the flows' series align with it.
    pub t: Vec<i64>,
    pub flows: Vec<FlowHistory>,
    /// Why there is nothing, when there is nothing.
    pub error: Option<String>,
}

/// `GET /api/overview`: every recorded receiver's latency over the last
/// hour and day, for the overview page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overview {
    /// Unix seconds the summary ends at.
    pub now: i64,
    pub receivers: Vec<ReceiverSummary>,
}

/// One receiver's latency, summed over its flows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverSummary {
    pub device: String,
    /// Flows recorded in the last hour.
    pub flows: u16,
    /// The largest per-second peak in the last hour, µs, and the latency of
    /// the flow it came from.
    pub peak_us: u32,
    pub latency_us: u32,
    /// Late packets in the last hour and the last day.
    pub late_1h: u64,
    pub late_24h: u64,
    /// Seconds with late packets in the last hour and the last 5 minutes:
    /// how much of the time was hit, not how many packets (one late packet
    /// is a fraction of a millisecond of audio).
    pub late_s_1h: u64,
    pub late_s_5m: u64,
    /// The last hour in [`SPARK_POINTS`] buckets: the largest peak over all
    /// flows as a share of that flow's latency, per mille (1000 = at the
    /// latency); `None` with nothing recorded.
    pub spark: Vec<Option<u32>>,
    /// Seconds since the last sample.
    pub age_s: Option<i64>,
}

/// Buckets in a receiver's overview sparkline.
pub const SPARK_POINTS: i64 = 60;

/// The most buckets a history answer has.
pub const HISTORY_POINTS: i64 = 1500;

/// One RX flow's history: its description and a value per bucket of
/// [`History::t`] (`None`: nothing recorded then). A slot whose flow changed
/// inside the range comes as one entry per flow, each with its own samples.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowHistory {
    pub slot: u16,
    pub latency_us: u32,
    pub rate: u32,
    pub channels: Vec<String>,
    pub sources: Vec<String>,
    /// The largest per-second peak lateness in each bucket, µs.
    pub max_us: Vec<Option<u32>>,
    /// The mean of the per-second peaks in each bucket, µs.
    pub mean_us: Vec<Option<u32>>,
    /// Late packets in each bucket.
    pub late: Vec<u64>,
    /// Over the whole range: the largest peak, the mean, the late packets,
    /// and how many stored values (of `tier_s` each) passed the latency.
    pub peak_us: u32,
    pub avg_us: u32,
    pub late_total: u64,
    pub over: u64,
    /// The stored values' distribution over [`latency_bins`]'s edges.
    pub histogram: Vec<u64>,
}

/// The histogram's upper edges for a flow of latency `latency_us`, µs:
/// eight up to the latency, then 1.5× and 2×; one more bin holds the rest.
pub fn latency_bins(latency_us: u32) -> Vec<u32> {
    let l = latency_us.max(8);
    let mut edges: Vec<u32> = (1..=8).map(|k| l * k / 8).collect();
    edges.push(l + l / 2);
    edges.push(l * 2);
    edges
}

/// The bin of `value_us` among `edges` (`edges.len()` for over the last);
/// a value on an edge belongs below it.
pub fn latency_bin(edges: &[u32], value_us: u32) -> usize {
    edges.partition_point(|&e| e < value_us)
}

impl RxChannel {
    /// `true` when the subscription receives audio.
    pub fn receiving(&self) -> bool {
        self.state.as_deref() == Some("connected")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_round_trip_through_json() {
        let d = Devices {
            enabled: true,
            interface: Some("eth1".into()),
            error: None,
            devices: vec![Device {
                name: "stage-box".into(),
                address: "10.0.0.5:4440".into(),
                manufacturer: Some("Yamaha".into()),
                model: Some("Rio".into()),
                channels: Some(Channels {
                    tx: vec![TxChannel {
                        channel: 1,
                        label: "Vox".into(),
                    }],
                    rx: vec![RxChannel {
                        channel: 1,
                        label: "In 1".into(),
                        source: Some(Source {
                            channel: "Vox".into(),
                            device: "desk".into(),
                        }),
                        status: Some("Subscribed (unicast)".into()),
                        state: Some("connected".into()),
                    }],
                }),
                settings: Some(Settings {
                    sample_rate: Some(48000),
                    latency_active_us: Some(1000),
                    ..Settings::default()
                }),
                info: Some(Info {
                    firmware_version: Some("4.2.5.1".into()),
                    encoding_bits: Some(24),
                    ..Info::default()
                }),
                clock: Some(Clock {
                    leader: true,
                    ports: vec!["leader".into()],
                    ..Clock::default()
                }),
                clock_error: None,
                network: Some(vec![NetPort {
                    link_mbit: 1000,
                    ip: "10.0.0.5".into(),
                    ..NetPort::default()
                }]),
                network_error: None,
                read_at: Some(1),
                error: None,
            }],
        };
        let text = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Devices>(&text).unwrap(), d);
        assert!(d.devices[0].channels.as_ref().unwrap().rx[0].receiving());
    }

    #[test]
    fn a_setting_change_reads_as_the_page_sends_it() {
        let c: SettingChange =
            serde_json::from_str(r#"{"device":"desk","setting":{"latency_us":500}}"#).unwrap();
        assert_eq!(c.setting, Setting::LatencyUs(500));
        assert_eq!(c.setting.what(), "latency");
        assert_eq!(Setting::SampleRateHz(48000).what(), "sample_rate");
        assert_eq!(Setting::EncodingBits(24).what(), "encoding");
        let back = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<SettingChange>(&back).unwrap(), c);
    }

    #[test]
    fn latency_bins_split_at_eighths_then_past_the_latency() {
        let e = latency_bins(1000);
        assert_eq!(e, vec![125, 250, 375, 500, 625, 750, 875, 1000, 1500, 2000]);
        assert_eq!(latency_bin(&e, 0), 0);
        assert_eq!(latency_bin(&e, 125), 0);
        assert_eq!(latency_bin(&e, 126), 1);
        assert_eq!(latency_bin(&e, 1000), 7);
        assert_eq!(latency_bin(&e, 1001), 8);
        assert_eq!(latency_bin(&e, 5000), 10);
        assert_eq!(latency_bins(0).len(), 10);
    }

    #[test]
    fn only_the_connected_state_counts_as_receiving() {
        let mut rx = RxChannel::default();
        assert!(!rx.receiving());
        rx.state = Some("pending".into());
        assert!(!rx.receiving());
        rx.state = Some("connected".into());
        assert!(rx.receiving());
    }
}
