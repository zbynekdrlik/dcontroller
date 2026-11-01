//! The routing matrix model: which RX channels take which TX channels, at
//! device or channel level, from the server's device list. Pure, so it is
//! tested on the host.

use dcontroller_api::{Device, Devices, RxChannel};

/// A transmitter: a device with TX channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tx {
    pub device: String,
    pub channels: Vec<String>,
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
    /// `n` subscriptions, `bad` of them not receiving.
    Warn { n: usize, bad: usize },
}

impl Cell {
    pub fn count(self) -> usize {
        match self {
            Cell::Empty => 0,
            Cell::Ok(n) | Cell::Warn { n, .. } => n,
        }
    }

    /// The CSS class.
    pub fn class(self) -> &'static str {
        match self {
            Cell::Empty => "cell",
            Cell::Ok(_) => "cell ok",
            Cell::Warn { .. } => "cell warn",
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
    let (mut n, mut bad) = (0, 0);
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
    }
    match (n, bad) {
        (0, _) => Cell::Empty,
        (n, 0) => Cell::Ok(n),
        (n, bad) => Cell::Warn { n, bad },
    }
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
    fn grid_takes_read_devices_with_channels_only() {
        let g = Grid::of(&site());
        assert_eq!(
            g.tx,
            vec![Tx {
                device: "desk".into(),
                channels: vec!["Mix L".into(), "Mix R".into()]
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
    fn subscriptions_to_unknown_transmitters_are_listed_apart() {
        let g = Grid::of(&site());
        assert_eq!(
            elsewhere(&g, &g.rx[0]),
            vec![("In 4", "Vox @ gone".to_string())]
        );
    }
}
