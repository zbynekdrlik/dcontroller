//! The routing matrix model: which RX channels take which TX channels, at
//! device or channel level, from the server's device list. Pure, so it is
//! tested on the host.

use dcontroller_api::{Device, Devices, RxChannel};

/// A transmitter: a device with TX channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tx {
    pub device: String,
    pub channels: Vec<String>,
    /// The channel numbers, in the order of `channels`.
    pub numbers: Vec<u16>,
}

/// A receiver: a device with RX channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rx {
    pub device: String,
    pub channels: Vec<RxChannel>,
}

/// The matrix: transmitters as columns, receivers as rows, both in the
/// server's order (by name).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grid {
    pub tx: Vec<Tx>,
    pub rx: Vec<Rx>,
}

/// What a cross-point shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// No subscription.
    Empty,
    /// `n` subscriptions, all receiving.
    Ok(usize),
    /// `n` subscriptions, `bad` of them not receiving, none failing.
    Warn { n: usize, bad: usize },
    /// `n` subscriptions, `bad` of them not receiving, at least one of
    /// those an error Dante reports (such as no free flow): the alarm.
    Err { n: usize, bad: usize },
}

impl Cell {
    pub fn count(self) -> usize {
        match self {
            Cell::Empty => 0,
            Cell::Ok(n) | Cell::Warn { n, .. } | Cell::Err { n, .. } => n,
        }
    }

    /// The CSS class.
    pub fn class(self) -> &'static str {
        match self {
            Cell::Empty => "cell",
            Cell::Ok(_) => "cell ok",
            Cell::Warn { .. } => "cell warn",
            Cell::Err { .. } => "cell err",
        }
    }
}

/// Dante compares device and channel names case aside.
fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl Grid {
    /// The grid of the devices whose channels are read.
    pub fn of(d: &Devices) -> Grid {
        let read = || {
            d.devices
                .iter()
                .filter_map(|dev: &Device| dev.channels.as_ref().map(|c| (dev, c)))
        };
        Grid {
            tx: read()
                .filter(|(_, c)| !c.tx.is_empty())
                .map(|(dev, c)| Tx {
                    device: dev.name.clone(),
                    channels: c.tx.iter().map(|t| t.label.clone()).collect(),
                    numbers: c.tx.iter().map(|t| t.channel).collect(),
                })
                .collect(),
            rx: read()
                .filter(|(_, c)| !c.rx.is_empty())
                .map(|(dev, c)| Rx {
                    device: dev.name.clone(),
                    channels: c.rx.clone(),
                })
                .collect(),
        }
    }
}

/// The cross-point of `rx` channels (a whole device, or one channel) and
/// transmitter `tx_device`, all its channels (`tx_channel` `None`) or one.
pub fn cell<'a>(
    rx: impl IntoIterator<Item = &'a RxChannel>,
    tx_device: &str,
    tx_channel: Option<&str>,
) -> Cell {
    let (mut n, mut bad, mut failing) = (0, 0, 0);
    for r in rx {
        let Some(s) = &r.source else { continue };
        if !same(&s.device, tx_device) {
            continue;
        }
        if tx_channel.is_some_and(|c| !same(&s.channel, c)) {
            continue;
        }
        n += 1;
        if !r.receiving() {
            bad += 1;
        }
        if r.state.as_deref() == Some("error") {
            failing += 1;
        }
    }
    match (n, bad, failing) {
        (0, _, _) => Cell::Empty,
        (n, 0, _) => Cell::Ok(n),
        (n, bad, 0) => Cell::Warn { n, bad },
        (n, bad, _) => Cell::Err { n, bad },
    }
}

/// What RX channel `r` takes, as `(TX device lower case, TX channel
/// number)` from the grid, when its source is a transmitter in it.
pub fn source_of(grid: &Grid, r: &RxChannel) -> Option<(String, u16)> {
    let s = r.source.as_ref()?;
    let t = grid.tx.iter().find(|t| same(&t.device, &s.device))?;
    let i = t.channels.iter().position(|c| same(c, &s.channel))?;
    Some((t.device.to_ascii_lowercase(), t.numbers[i]))
}

/// The RX channels (as `label @ device`) whose subscription names TX
/// channel `tx_channel` of `tx_device` (`None`: any of its channels): what
/// a rename of that channel or device leaves unresolved.
pub fn affected(d: &Devices, tx_device: &str, tx_channel: Option<&str>) -> Vec<String> {
    d.devices
        .iter()
        .filter_map(|dev| dev.channels.as_ref().map(|c| (dev, c)))
        .flat_map(|(dev, c)| {
            c.rx.iter().filter_map(move |r| {
                let s = r.source.as_ref()?;
                let hit =
                    same(&s.device, tx_device) && tx_channel.is_none_or(|t| same(&s.channel, t));
                hit.then(|| format!("{} @ {}", r.label, dev.name))
            })
        })
        .collect()
}

/// Subscriptions of `rx` to devices that are not transmitters in the grid
/// (offline, or not read yet): (RX channel label, source text).
pub fn elsewhere<'a>(grid: &Grid, rx: &'a Rx) -> Vec<(&'a str, String)> {
    rx.channels
        .iter()
        .filter_map(|r| {
            let s = r.source.as_ref()?;
            let known = grid.tx.iter().any(|t| {
                same(&t.device, &s.device) && t.channels.iter().any(|c| same(c, &s.channel))
            });
            (!known).then(|| (r.label.as_str(), format!("{} @ {}", s.channel, s.device)))
        })
        .collect()
}

/// A subscription that is not receiving: which RX channel, from where,
/// and Dante's status for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// `label @ device` of the RX channel.
    pub rx: String,
    /// `channel @ device` it asks for.
    pub source: String,
    /// Dante's status text, such as "No more unicast flows (Tx)".
    pub status: String,
    pub kind: Kind,
}

/// Why a subscription does not receive, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Dante reports an error, such as no free flow on the transmitter:
    /// the alarm.
    Failing,
    /// The source device is on the network, but the channel is not found
    /// or the flow is not up yet.
    Unresolved,
    /// The source device is not on the network (switched off, on
    /// purpose or not): no error.
    Offline,
}

impl Kind {
    pub fn class(self) -> &'static str {
        match self {
            Kind::Failing => "problem err",
            Kind::Unresolved => "problem warn",
            Kind::Offline => "problem off",
        }
    }
}

/// A receiver's count for its row header: (failing, unresolved) among its
/// subscriptions; a source that is off the network counts in neither.
pub fn trouble(d: &Devices, rx: &Rx) -> (usize, usize) {
    let online = |name: &str| d.devices.iter().any(|x| same(&x.name, name));
    rx.channels
        .iter()
        .filter(|r| r.source.is_some() && !r.receiving())
        .fold((0, 0), |(f, u), r| {
            if r.state.as_deref() == Some("error") {
                (f + 1, u)
            } else if r.source.as_ref().is_some_and(|s| online(&s.device)) {
                (f, u + 1)
            } else {
                (f, u)
            }
        })
}

/// The one-line count of [`problems`]: (failing, unresolved, offline).
pub fn tally(p: &[Problem]) -> (usize, usize, usize) {
    let n = |k| p.iter().filter(|x| x.kind == k).count();
    (n(Kind::Failing), n(Kind::Unresolved), n(Kind::Offline))
}

/// Every subscription of the read devices that is not receiving, worst
/// kind first, then by receiver in the server's order.
pub fn problems(d: &Devices) -> Vec<Problem> {
    let online = |name: &str| d.devices.iter().any(|x| same(&x.name, name));
    let mut out: Vec<Problem> = d
        .devices
        .iter()
        .filter_map(|dev| dev.channels.as_ref().map(|c| (dev, c)))
        .flat_map(|(dev, c)| {
            c.rx.iter().filter_map(move |r| {
                let s = r.source.as_ref()?;
                (!r.receiving()).then(|| Problem {
                    rx: format!("{} @ {}", r.label, dev.name),
                    source: format!("{} @ {}", s.channel, s.device),
                    status: r.status.clone().unwrap_or_else(|| "no status".into()),
                    kind: if r.state.as_deref() == Some("error") {
                        Kind::Failing
                    } else if online(&s.device) {
                        Kind::Unresolved
                    } else {
                        Kind::Offline
                    },
                })
            })
        })
        .collect();
    out.sort_by_key(|p| p.kind);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcontroller_api::{Channels, Source, TxChannel};

    fn rx(label: &str, src: Option<(&str, &str)>, status: &str) -> RxChannel {
        RxChannel {
            channel: 1,
            label: label.into(),
            source: src.map(|(c, d)| Source {
                channel: c.into(),
                device: d.into(),
            }),
            status: src.map(|_| status.to_string()),
            state: src.map(|_| {
                if status == "receiving" {
                    "connected"
                } else {
                    "pending"
                }
                .to_string()
            }),
        }
    }

    fn device(name: &str, tx: &[&str], rx: Vec<RxChannel>) -> Device {
        Device {
            name: name.into(),
            channels: Some(Channels {
                tx: tx
                    .iter()
                    .enumerate()
                    .map(|(i, l)| TxChannel {
                        channel: i as u16 + 1,
                        label: l.to_string(),
                    })
                    .collect(),
                rx,
            }),
            ..Device::default()
        }
    }

    fn site() -> Devices {
        Devices {
            started_at: 0,
            enabled: true,
            interface: Some("eth1".into()),
            error: None,
            devices: vec![
                device("desk", &["Mix L", "Mix R"], vec![]),
                device(
                    "stage",
                    &[],
                    vec![
                        rx("In 1", Some(("Mix L", "desk")), "receiving"),
                        rx("In 2", Some(("mix r", "DESK")), "unresolved"),
                        rx("In 3", None, ""),
                        rx("In 4", Some(("Vox", "gone")), "unresolved"),
                    ],
                ),
                Device {
                    name: "unread".into(),
                    ..Device::default()
                },
            ],
        }
    }

    #[test]
    fn problems_list_subscriptions_not_receiving_errors_first() {
        let mut d = site();
        let mut flows = rx("In 9", Some(("Mix R", "desk")), "x");
        flows.status = Some("No more unicast flows (Tx)".into());
        flows.state = Some("error".into());
        d.devices.push(device("iem", &[], vec![flows]));
        let p = problems(&d);
        assert_eq!(
            p,
            vec![
                Problem {
                    rx: "In 9 @ iem".into(),
                    source: "Mix R @ desk".into(),
                    status: "No more unicast flows (Tx)".into(),
                    kind: Kind::Failing,
                },
                Problem {
                    rx: "In 2 @ stage".into(),
                    source: "mix r @ DESK".into(),
                    status: "unresolved".into(),
                    kind: Kind::Unresolved,
                },
                // "gone" is not on the network: offline, no error.
                Problem {
                    rx: "In 4 @ stage".into(),
                    source: "Vox @ gone".into(),
                    status: "unresolved".into(),
                    kind: Kind::Offline,
                },
            ]
        );
        assert_eq!(tally(&p), (1, 1, 1));
        // Receiving and unsubscribed channels are no problem.
        assert!(
            p.iter()
                .all(|x| !x.rx.starts_with("In 1") && !x.rx.starts_with("In 3"))
        );
    }

    #[test]
    fn grid_takes_read_devices_with_channels_only() {
        let g = Grid::of(&site());
        assert_eq!(
            g.tx,
            vec![Tx {
                device: "desk".into(),
                channels: vec!["Mix L".into(), "Mix R".into()],
                numbers: vec![1, 2],
            }]
        );
        assert_eq!(g.rx.len(), 1);
        assert_eq!(g.rx[0].device, "stage");
        assert_eq!(g.rx[0].channels.len(), 4);
    }

    #[test]
    fn device_cell_counts_subscriptions_case_aside_and_flags_not_receiving() {
        let g = Grid::of(&site());
        let c = cell(&g.rx[0].channels, "desk", None);
        assert_eq!(c, Cell::Warn { n: 2, bad: 1 });
        assert_eq!(c.count(), 2);
        assert_eq!(c.class(), "cell warn");
        assert_eq!(cell(&g.rx[0].channels, "nobody", None), Cell::Empty);
    }

    #[test]
    fn a_failing_subscription_makes_the_cross_point_red() {
        let mut f = rx("Vox", Some(("Mix L", "desk")), "No more flows (Tx)");
        f.state = Some("error".into());
        let ok = rx("Vox 2", Some(("Mix R", "desk")), "receiving");
        let c = cell(&[f.clone(), ok.clone()], "desk", None);
        assert_eq!(c, Cell::Err { n: 2, bad: 1 });
        assert_eq!(c.class(), "cell err");
        assert_eq!(c.count(), 2);
        // Only a not-yet-resolved one stays amber.
        let pending = rx("Vox 3", Some(("Mix L", "desk")), "Unresolved");
        assert_eq!(cell(&[pending, ok], "desk", None).class(), "cell warn");
    }

    #[test]
    fn a_receiver_header_counts_failing_and_unresolved_but_not_offline_sources() {
        let mut f = rx("A", Some(("Mix L", "desk")), "No more flows (Tx)");
        f.state = Some("error".into());
        let r = Rx {
            device: "stage".into(),
            channels: vec![
                f,
                rx("B", Some(("Nowhere", "desk")), "Unresolved"),
                rx("C", Some(("Mic", "switched-off")), "Unresolved"),
                rx("D", Some(("Mix R", "desk")), "receiving"),
                rx("E", None, ""),
            ],
        };
        let d = Devices {
            devices: vec![device("desk", &["Mix L", "Mix R"], vec![])],
            ..Devices::default()
        };
        assert_eq!(trouble(&d, &r), (1, 1));
    }

    #[test]
    fn an_rx_channel_s_source_is_found_in_the_grid_case_aside() {
        let g = Grid::of(&site());
        let r = rx("Vox", Some(("MIX R", "Desk")), "receiving");
        assert_eq!(source_of(&g, &r), Some(("desk".into(), 2)));
        assert_eq!(
            source_of(&g, &rx("Vox", Some(("Nowhere", "desk")), "x")),
            None
        );
        assert_eq!(source_of(&g, &rx("Vox", None, "")), None);
    }

    #[test]
    fn channel_cells_match_one_tx_channel() {
        let g = Grid::of(&site());
        let ch = &g.rx[0].channels;
        assert_eq!(cell(&ch[0..1], "desk", Some("Mix L")), Cell::Ok(1));
        assert_eq!(cell(&ch[0..1], "desk", Some("Mix R")), Cell::Empty);
        assert_eq!(
            cell(&ch[1..2], "desk", Some("Mix R")),
            Cell::Warn { n: 1, bad: 1 }
        );
        assert_eq!(cell(&ch[2..3], "desk", None), Cell::Empty);
        assert_eq!(cell(ch, "desk", Some("Mix L")), Cell::Ok(1));
        assert_eq!(Cell::Ok(1).class(), "cell ok");
        assert_eq!(Cell::Empty.class(), "cell");
    }

    #[test]
    fn affected_lists_the_receivers_naming_a_channel_or_device() {
        let d = site();
        assert_eq!(
            affected(&d, "desk", Some("Mix L")),
            vec!["In 1 @ stage".to_string()]
        );
        assert_eq!(
            affected(&d, "DESK", Some("MIX R")),
            vec!["In 2 @ stage".to_string()]
        );
        assert_eq!(affected(&d, "desk", None).len(), 2);
        assert!(affected(&d, "desk", Some("Talkback")).is_empty());
        assert!(affected(&d, "nobody", None).is_empty());
    }

    #[test]
    fn subscriptions_to_unknown_transmitters_are_listed_apart() {
        let g = Grid::of(&site());
        assert_eq!(
            elsewhere(&g, &g.rx[0]),
            vec![("In 4", "Vox @ gone".to_string())]
        );
    }
}
