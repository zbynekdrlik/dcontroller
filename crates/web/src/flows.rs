//! A transmitter's TX flows on the page: rows with the receivers named by
//! the device that holds their address, and the create question. Pure, so
//! it is tested on the host.

use dcontroller_api::{Devices, TxFlow};

/// At most this many channels in one flow (dstack `MAX_FLOW_CHANNELS`).
pub const MAX_CHANNELS: usize = 8;

/// One flow as a table row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub slot: u16,
    pub multicast: bool,
    /// The channels, `label (n)` each.
    pub channels: String,
    /// Where it goes: a group, or the receiving devices by name.
    pub to: String,
}

/// The device listed at the destination's IP (its address is `ip:port`).
/// Several devices on one PC share an IP and the audio port does not tell
/// them apart: then all their names and the IP. None: the destination.
fn holder(d: &Devices, dest: &str) -> String {
    let ip = dest.rsplit_once(':').map_or(dest, |(ip, _)| ip);
    let names: Vec<&str> = d
        .devices
        .iter()
        .filter(|x| {
            x.address
                .rsplit_once(':')
                .map_or(x.address.as_str(), |(a, _)| a)
                == ip
        })
        .map(|x| x.name.as_str())
        .collect();
    match names.as_slice() {
        [] => dest.to_owned(),
        [one] => (*one).to_owned(),
        many => format!("{} ({ip})", many.join(" / ")),
    }
}

/// The label of TX channel `n` of `tx`, else its number.
fn label(tx: &[(u16, String)], n: u16) -> String {
    tx.iter()
        .find(|(c, _)| *c == n)
        .map_or_else(|| n.to_string(), |(_, l)| format!("{l} ({n})"))
}

pub fn rows(flows: &[TxFlow], tx: &[(u16, String)], d: &Devices) -> Vec<Row> {
    flows
        .iter()
        .map(|f| Row {
            slot: f.slot,
            multicast: f.multicast,
            channels: f
                .channels
                .iter()
                // 0: an empty channel slot of the flow.
                .filter(|&&n| n != 0)
                .map(|&n| label(tx, n))
                .collect::<Vec<_>>()
                .join(", "),
            to: if f.multicast {
                f.destinations.join(" ")
            } else {
                f.destinations
                    .iter()
                    .map(|x| holder(d, x))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        })
        .collect()
}

/// "flows N/max" as the page writes it, and whether every slot is taken
/// (the next unicast receiver gets "No more flows"). A device that states
/// no slot count gets the count alone.
pub fn count_text(n: usize, slots: u16, unread: usize) -> (String, bool) {
    let mut t = if slots == 0 {
        format!("{n} TX flow{}", if n == 1 { "" } else { "s" })
    } else {
        format!("{n} of {slots} TX flows")
    };
    if unread > 0 {
        t.push_str(&format!(" ({unread} not readable)"));
    }
    let full = slots > 0 && n + unread >= usize::from(slots);
    if full {
        t.push_str(": all taken");
    }
    (t, full)
}

/// The confirmation question for a new multicast flow.
pub fn question(device: &str, chosen: &[u16], tx: &[(u16, String)]) -> String {
    let ch = chosen
        .iter()
        .map(|&n| label(tx, n))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Create a multicast flow on {device} with {ch}? It takes one of the device's TX flows; every receiver can then share it."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcontroller_api::Device;

    fn devices() -> Devices {
        let d = |name: &str, address: &str| Device {
            name: name.into(),
            address: address.into(),
            ..Device::default()
        };
        Devices {
            devices: vec![
                d("band", "10.0.0.30:4441"),
                d("iem", "10.0.0.31:4440"),
                d("strih", "10.0.0.40:4440"),
                d("intercom", "10.0.0.40:4441"),
            ],
            ..Devices::default()
        }
    }

    fn tx() -> Vec<(u16, String)> {
        vec![(1, "HAND 1".into()), (2, "PETA".into())]
    }

    #[test]
    fn a_unicast_flow_names_its_receiver_and_a_multicast_its_group() {
        let flows = vec![
            TxFlow {
                id: 1,
                slot: 1,
                multicast: false,
                channels: vec![1, 0, 0, 0],
                destinations: vec![
                    "10.0.0.30:14336".into(),
                    "10.0.0.99:4321".into(),
                    "10.0.0.40:14400".into(),
                ],
            },
            TxFlow {
                id: 2,
                slot: 2,
                multicast: true,
                channels: vec![1, 2],
                destinations: vec!["239.255.1.2:4321".into()],
            },
        ];
        let r = rows(&flows, &tx(), &devices());
        assert_eq!(
            r[0].to,
            "band, 10.0.0.99:4321, strih / intercom (10.0.0.40)"
        );
        assert_eq!(r[0].channels, "HAND 1 (1)");
        assert_eq!(r[1].to, "239.255.1.2:4321");
        assert_eq!(r[1].channels, "HAND 1 (1), PETA (2)");
        assert!(r[1].multicast);
    }

    #[test]
    fn the_count_reads_n_of_max_and_flags_a_full_transmitter() {
        assert_eq!(count_text(1, 2, 0), ("1 of 2 TX flows".to_string(), false));
        assert_eq!(
            count_text(2, 2, 0),
            ("2 of 2 TX flows: all taken".to_string(), true)
        );
        assert!(count_text(1, 2, 1).1);
        assert_eq!(count_text(1, 0, 0), ("1 TX flow".to_string(), false));
        assert_eq!(count_text(3, 0, 1).0, "3 TX flows (1 not readable)");
    }

    #[test]
    fn the_question_names_device_and_channels() {
        let q = question("mics", &[1, 3], &tx());
        assert!(q.starts_with("Create a multicast flow on mics with HAND 1 (1), 3?"));
    }
}
