//! The Changes page's rows (dcontroller #123): the log holds one entry per
//! RX channel per status flip, so a connection that dropped and came back
//! is four entries for a stereo pair. Here they fold into one outage per
//! connection (source device into receiver), from its stop to its return;
//! every other entry stays a row of its own. Pure, tested on the host.

use dcontroller_api::ChangeEntry;

/// One connection not receiving, from its first channel's stop to its
/// last channel's return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outage {
    /// The source device and the receiver.
    pub from: String,
    pub to: String,
    /// The channels hit, as `RX label ← TX channel`, in the order they stopped.
    pub channels: Vec<String>,
    /// Unix seconds; `None` when the log holds only the return.
    pub start: Option<u64>,
    /// When every channel received again; `None` while one does not.
    pub end: Option<u64>,
    /// Why, as the devices said it, each once.
    pub reasons: Vec<String>,
}

impl Outage {
    /// When it last changed: where it sorts.
    pub fn at(&self) -> u64 {
        self.end.or(self.start).unwrap_or(0)
    }

    /// How long it lasted, when both ends are known.
    pub fn seconds(&self) -> Option<u64> {
        Some(self.end?.saturating_sub(self.start?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// A change someone made, here or outside dcontroller.
    Change(ChangeEntry),
    /// A network alarm's start or end (mDNS sender, 96 s browser, floor).
    Alarm(ChangeEntry),
    Outage(Outage),
}

impl Item {
    pub fn at(&self) -> u64 {
        match self {
            Item::Change(e) | Item::Alarm(e) => e.at,
            Item::Outage(o) => o.at(),
        }
    }

    /// Whether it concerns the device `name` (#142: a connection's "Changes
    /// of" link): as the receiver, or as a source before, asked or after.
    pub fn concerns(&self, name: &str) -> bool {
        let is = |n: &str| n.eq_ignore_ascii_case(name);
        match self {
            Item::Change(e) | Item::Alarm(e) => {
                is(&e.rx_device)
                    || [&e.before, &e.requested, &e.after]
                        .into_iter()
                        .flatten()
                        .any(|s| is(&s.device))
            }
            Item::Outage(o) => is(&o.from) || is(&o.to),
        }
    }

    /// Whether the device-status filter shows it.
    pub fn is_status(&self) -> bool {
        !matches!(self, Item::Change(_))
    }
}

/// A subscription's status flip (not a network alarm).
fn flip(e: &ChangeEntry) -> bool {
    e.kind == "status" && e.what == "rx"
}

/// `entries` (newest first, as the server lists them) as rows, newest
/// first: status flips folded into outages, the rest one to one.
pub fn fold(entries: &[ChangeEntry]) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    // Per connection: its open outage's place, and the channels still down.
    let mut open: Vec<((String, String), usize, Vec<u16>)> = Vec::new();
    for e in entries.iter().rev() {
        if !flip(e) {
            items.push(if e.kind == "status" {
                Item::Alarm(e.clone())
            } else {
                Item::Change(e.clone())
            });
            continue;
        }
        let source = e.after.as_ref().or(e.before.as_ref());
        let from = source.map_or_else(|| "?".to_owned(), |s| s.device.clone());
        let key = (from.to_lowercase(), e.rx_device.to_lowercase());
        let channel = format!(
            "{} ← {}",
            e.rx_label,
            source.map_or("?", |s| s.channel.as_str())
        );
        let at = open.iter().position(|(k, _, _)| *k == key);
        if e.ok {
            // A return: of the open outage, else of one the log lost.
            match at {
                Some(i) => {
                    let (_, item, down) = &mut open[i];
                    down.retain(|c| *c != e.rx_channel);
                    if down.is_empty() {
                        if let Item::Outage(o) = &mut items[*item] {
                            o.end = Some(e.at);
                        }
                        open.remove(i);
                    }
                }
                None => items.push(Item::Outage(Outage {
                    from,
                    to: e.rx_device.clone(),
                    channels: vec![channel],
                    start: None,
                    end: Some(e.at),
                    reasons: Vec::new(),
                })),
            }
            continue;
        }
        let reason = e
            .message
            .strip_prefix("stopped receiving: ")
            .unwrap_or(&e.message)
            .to_owned();
        match at {
            Some(i) => {
                let (_, item, down) = &mut open[i];
                if !down.contains(&e.rx_channel) {
                    down.push(e.rx_channel);
                }
                if let Item::Outage(o) = &mut items[*item] {
                    if !o.channels.contains(&channel) {
                        o.channels.push(channel);
                    }
                    if !o.reasons.contains(&reason) {
                        o.reasons.push(reason);
                    }
                }
            }
            None => {
                items.push(Item::Outage(Outage {
                    from,
                    to: e.rx_device.clone(),
                    channels: vec![channel],
                    start: Some(e.at),
                    end: None,
                    reasons: vec![reason],
                }));
                open.push((key, items.len() - 1, vec![e.rx_channel]));
            }
        }
    }
    // Newest first; equals keep the log's order (newest entry first).
    items.reverse();
    items.sort_by_key(|i| std::cmp::Reverse(i.at()));
    items
}

/// A span in words: `45 s`, `2 min 5 s`, `1 h 3 min`.
pub fn span(seconds: u64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => match s % 60 {
            0 => format!("{} min", s / 60),
            r => format!("{} min {r} s", s / 60),
        },
        s => match s % 3600 / 60 {
            0 => format!("{} h", s / 3600),
            m => format!("{} h {m} min", s / 3600),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcontroller_api::Source;

    fn status(at: u64, rx: &str, ch: u16, from: &str, ok: bool, why: &str) -> ChangeEntry {
        let src = Some(Source {
            channel: format!("Out {ch}"),
            device: from.into(),
        });
        ChangeEntry {
            at,
            user: "device status".into(),
            kind: "status".into(),
            what: "rx".into(),
            rx_device: rx.into(),
            rx_channel: ch,
            rx_label: format!("In {ch}"),
            before: src.clone(),
            after: src,
            ok,
            message: if ok {
                "receiving again".into()
            } else {
                format!("stopped receiving: {why}")
            },
            ..ChangeEntry::default()
        }
    }

    fn change(at: u64, kind: &str) -> ChangeEntry {
        ChangeEntry {
            at,
            user: "owner".into(),
            kind: kind.into(),
            ok: true,
            ..ChangeEntry::default()
        }
    }

    /// As the server lists them: newest first (given in the order they
    /// were logged).
    fn log(mut v: Vec<ChangeEntry>) -> Vec<ChangeEntry> {
        v.reverse();
        v
    }

    #[test]
    fn a_stereo_pair_that_drops_and_returns_is_one_outage() {
        let entries = log(vec![
            status(100, "amp", 1, "desk", false, "Unresolved"),
            status(100, "amp", 2, "desk", false, "Unresolved"),
            change(110, "rename"),
            status(130, "amp", 1, "desk", true, ""),
            status(131, "amp", 2, "DESK", true, ""),
        ]);
        let items = fold(&entries);
        assert_eq!(items.len(), 2);
        let Item::Outage(o) = &items[0] else {
            panic!("the outage ended last, so it is on top: {items:?}");
        };
        assert_eq!((o.from.as_str(), o.to.as_str()), ("desk", "amp"));
        assert_eq!(o.channels, ["In 1 ← Out 1", "In 2 ← Out 2"]);
        assert_eq!(
            (o.start, o.end, o.seconds()),
            (Some(100), Some(131), Some(31))
        );
        assert_eq!(o.reasons, ["Unresolved"]);
        assert!(matches!(&items[1], Item::Change(e) if e.kind == "rename"));
        assert!(items[0].is_status() && !items[1].is_status());
    }

    #[test]
    fn an_outage_stays_open_until_its_last_channel_returns_and_keeps_every_reason() {
        let entries = log(vec![
            status(100, "amp", 1, "desk", false, "Unresolved"),
            status(105, "amp", 2, "desk", false, "Transmitter setup failed"),
            status(120, "amp", 1, "desk", true, ""),
            // Another connection of the same receiver is its own outage.
            status(121, "amp", 3, "player", false, "No more flows (Tx)"),
        ]);
        let items = fold(&entries);
        assert_eq!(items.len(), 2);
        let Item::Outage(newer) = &items[0] else {
            panic!()
        };
        assert_eq!(
            (newer.from.as_str(), newer.start, newer.end),
            ("player", Some(121), None)
        );
        let Item::Outage(o) = &items[1] else { panic!() };
        assert_eq!((o.start, o.end, o.seconds()), (Some(100), None, None));
        assert_eq!(o.reasons, ["Unresolved", "Transmitter setup failed"]);
        assert_eq!(o.at(), 100);
    }

    #[test]
    fn a_second_drop_of_the_same_connection_is_a_second_outage() {
        let entries = log(vec![
            status(100, "amp", 1, "desk", false, "Unresolved"),
            status(110, "amp", 1, "desk", true, ""),
            status(200, "amp", 1, "desk", false, "Unresolved"),
            status(260, "amp", 1, "desk", true, ""),
        ]);
        let spans: Vec<Option<u64>> = fold(&entries)
            .iter()
            .map(|i| match i {
                Item::Outage(o) => o.seconds(),
                _ => None,
            })
            .collect();
        assert_eq!(spans, [Some(60), Some(10)]);
    }

    #[test]
    fn a_return_whose_stop_the_log_lost_and_alarms_stay_rows_of_their_own() {
        let mut alarm = change(50, "status");
        alarm.what = "mdns".into();
        let entries = log(vec![alarm.clone(), status(60, "amp", 1, "desk", true, "")]);
        let items = fold(&entries);
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0], Item::Outage(o) if o.start.is_none() && o.end == Some(60)));
        assert_eq!(items[1], Item::Alarm(alarm));
        assert!(items[1].is_status());
        assert!(fold(&[]).is_empty());
    }

    #[test]
    fn an_item_concerns_its_receiver_and_its_sources() {
        let items = fold(&log(vec![
            status(100, "amp", 1, "desk", false, "Unresolved"),
            change(110, "rename"),
        ]));
        let Item::Outage(_) = &items[1] else { panic!() };
        assert!(items[1].concerns("AMP") && items[1].concerns("desk"));
        assert!(!items[1].concerns("player"));
        // A change with no device named concerns none.
        assert!(!items[0].concerns("amp"));
        let mut sub = change(120, "subscribe");
        sub.rx_device = "rec".into();
        sub.after = Some(Source {
            channel: "Out 1".into(),
            device: "Player".into(),
        });
        let i = Item::Change(sub);
        assert!(i.concerns("rec") && i.concerns("player") && !i.concerns("desk"));
    }

    #[test]
    fn spans_read_in_the_largest_two_units() {
        assert_eq!(span(0), "0 s");
        assert_eq!(span(45), "45 s");
        assert_eq!(span(120), "2 min");
        assert_eq!(span(125), "2 min 5 s");
        assert_eq!(span(3600), "1 h");
        assert_eq!(span(3790), "1 h 3 min");
    }
}
