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

/// One logged change (`GET /api/changes`, newest first).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEntry {
    /// Unix seconds.
    pub at: u64,
    /// The Cloudflare Access e-mail, or `local` without Access.
    pub user: String,
    pub rx_device: String,
    pub rx_channel: u16,
    pub rx_label: String,
    /// The subscription before, as read before the change.
    pub before: Option<Source>,
    /// The subscription asked for (`None` = remove).
    pub requested: Option<Source>,
    /// The subscription read back after the change.
    pub after: Option<Source>,
    pub ok: bool,
    pub message: String,
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
