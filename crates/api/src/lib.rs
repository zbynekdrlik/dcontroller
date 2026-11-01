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
    /// Unix seconds of the last complete read.
    pub read_at: Option<u64>,
    /// The last read's error, when it failed (the previous read stays).
    pub error: Option<String>,
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
    /// `subscription` (also when empty, as in the first log lines) or `rename`.
    pub kind: String,
    /// For a rename: `tx`, `rx` or `device`.
    pub what: String,
    /// The device the change went to (for a subscription, the receiver).
    pub rx_device: String,
    /// The channel (0 for a device rename).
    pub rx_channel: u16,
    pub rx_label: String,
    /// For a rename: the name before, asked for (`None` = default) and after.
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

/// `GET /api/latency/{device}`: the receiver's packet latency per RX flow,
/// as Dante Controller's Latency tab shows it, gathered from its heartbeats
/// while someone watches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyReport {
    pub device: String,
    /// Seconds the server has been listening to this device.
    pub watching_s: u64,
    /// Heartbeats heard (one a second).
    pub beats: u64,
    pub flows: Vec<FlowLatency>,
    /// Why there is no data, when there is none.
    pub error: Option<String>,
}

/// One RX flow of the receiver.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowLatency {
    /// The flow's slot, 1 up.
    pub slot: u16,
    /// The flow's latency, µs: a packet older than this plays late.
    pub latency_us: u32,
    pub rate: u32,
    /// The RX channels it feeds (labels).
    pub channels: Vec<String>,
    /// The transmitting devices of those channels.
    pub sources: Vec<String>,
    /// The largest packet latency of each second, µs, oldest first (at most
    /// the last ten minutes).
    pub peaks_us: Vec<u32>,
    /// Late packets since the server started listening.
    pub late: u64,
    /// Late packets since the device started (the device's own count).
    pub late_total: u32,
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
                read_at: Some(1),
                error: None,
            }],
        };
        let text = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Devices>(&text).unwrap(), d);
        assert!(d.devices[0].channels.as_ref().unwrap().rx[0].receiving());
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
