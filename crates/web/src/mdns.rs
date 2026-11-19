//! The mDNS senders on the Dante leg (dcontroller #86, dstack #289): the
//! rates on the Network page and the alarm on every page while a source
//! sends over the cap. Beside it the two alarms of dcontroller #93: a host
//! that refreshes Dante records every 96 s (dstack #319) and a dstack peer
//! below the network floor (dstack #321).

use dcontroller_api::{BelowFloor, Mdns, MdnsBrowser, MdnsSender};

/// A sender as `address (devices) n/s`.
pub fn sender_text(s: &MdnsSender) -> String {
    let who = if s.devices.is_empty() {
        s.source.clone()
    } else {
        format!("{} ({})", s.source, s.devices.join(", "))
    };
    format!("{who} {:.1}/s", s.per_s)
}

/// The alarm line, while a source is over the cap.
pub fn alarm(m: &Mdns) -> Option<String> {
    let over = &m.minute.as_ref()?.over_cap;
    if over.is_empty() {
        return None;
    }
    let who: Vec<String> = over.iter().map(sender_text).collect();
    Some(format!(
        "mDNS flood on the Dante leg: {} over the cap of {}/s",
        who.join(", "),
        m.cap_per_s
    ))
}

/// A 96 s browser as `address (devices) every n s`.
pub fn browser_text(b: &MdnsBrowser) -> String {
    let who = if b.devices.is_empty() {
        b.source.clone()
    } else {
        format!("{} ({})", b.source, b.devices.join(", "))
    };
    format!("{who} every {:.1} s", b.period_s)
}

/// The 96 s browser alarm line, while the last minute lists one.
pub fn browser_alarm(m: &Mdns) -> Option<String> {
    let b = &m.minute.as_ref()?.browsers;
    if b.is_empty() {
        return None;
    }
    let who: Vec<String> = b.iter().map(browser_text).collect();
    Some(format!(
        "Dante browsed on a 96 s cycle by {}: every Dante host answers it at once",
        who.join(", ")
    ))
}

/// A binary below the floor as `name at address dstack version`.
pub fn below_text(b: &BelowFloor) -> String {
    match (b.kind.as_str(), &b.address) {
        // A device announces a local client below the floor (dstack #323).
        ("client", Some(a)) => format!("local client of {} at {a} (dstack {})", b.name, b.version),
        (_, Some(a)) => format!("{} at {a} (dstack {})", b.name, b.version),
        (_, None) => format!("{} (local {}, dstack {})", b.name, b.kind, b.version),
    }
}

/// The network-floor alarm line, while a binary below it is listed.
pub fn floor_alarm(m: &Mdns) -> Option<String> {
    if m.below_floor.is_empty() {
        return None;
    }
    let who: Vec<String> = m.below_floor.iter().map(below_text).collect();
    Some(format!(
        "dstack below the network floor {}: {}: upgrade it",
        m.floor,
        who.join(", ")
    ))
}

/// The Network page's line on the 96 s browsers, and whether it warns.
pub fn browser_summary(m: &Mdns) -> (String, bool) {
    match m.minute.as_ref().map(|x| x.browsers.len()) {
        None => ("96 s browsers: none known yet".to_owned(), false),
        Some(0) => (
            "96 s browsers: no host refreshes Dante records on the 96 s cycle".to_owned(),
            false,
        ),
        Some(n) => (
            format!("96 s browsers: {n} host(s) refresh Dante records every 96 s"),
            true,
        ),
    }
}

/// The Network page's line on the network floor, and whether it warns.
pub fn floor_summary(m: &Mdns) -> (String, bool) {
    match m.below_floor.len() {
        0 => (
            format!(
                "dstack network floor {}: no device below it heard in the last hour",
                m.floor
            ),
            false,
        ),
        n => (
            format!(
                "dstack network floor {}: {n} device(s) below it heard in the last hour",
                m.floor
            ),
            true,
        ),
    }
}

/// The Network page's line on the mDNS meter, and whether it warns.
pub fn summary(m: &Mdns) -> (String, bool) {
    match (&m.minute, m.running) {
        (Some(min), _) => (
            format!(
                "mDNS on the Dante leg, last minute: {:.1}/s from all senders · cap {}/s a sender",
                min.per_s, m.cap_per_s
            ),
            !min.over_cap.is_empty(),
        ),
        (None, true) => (
            "mDNS on the Dante leg: counting, the first minute is not over yet".to_owned(),
            false,
        ),
        (None, false) => (
            format!(
                "mDNS on the Dante leg: not counted ({})",
                m.error.as_deref().unwrap_or("not started")
            ),
            true,
        ),
    }
}

/// The mDNS tile of the Map (#142): a word, a line and the tile's class
/// (`ok`, `warn`, `bad`). A flood, a 96 s browser or a binary below the
/// floor is `bad`, in that order; a meter that does not run is `warn`.
pub fn tile(m: &Mdns) -> (String, String, &'static str) {
    let Some(min) = &m.minute else {
        return if m.running {
            (
                "counting".into(),
                "the first minute is not over yet".into(),
                "ok",
            )
        } else {
            (
                "not counted".into(),
                m.error.clone().unwrap_or_else(|| "not started".into()),
                "warn",
            )
        };
    };
    if let Some(s) = min.over_cap.first() {
        let more = match min.over_cap.len() {
            1 => String::new(),
            n => format!(" and {} more", n - 1),
        };
        return (
            "flood".into(),
            format!("{}{more} over the cap of {}/s", sender_text(s), m.cap_per_s),
            "bad",
        );
    }
    if let Some(b) = min.browsers.first() {
        return ("96 s browsing".into(), browser_text(b), "bad");
    }
    if let Some(b) = m.below_floor.first() {
        return (
            "old dstack".into(),
            format!("{} below {}", below_text(b), m.floor),
            "bad",
        );
    }
    (
        "quiet".into(),
        format!(
            "{:.1}/s last minute · cap {}/s a sender",
            min.per_s, m.cap_per_s
        ),
        "ok",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcontroller_api::MdnsMinute;

    fn sender(source: &str, devices: &[&str], per_s: f64) -> MdnsSender {
        MdnsSender {
            source: source.into(),
            devices: devices.iter().map(|d| (*d).to_owned()).collect(),
            per_s,
        }
    }

    fn mdns(over: Vec<MdnsSender>) -> Mdns {
        Mdns {
            running: true,
            error: None,
            cap_per_s: 50,
            minute: Some(MdnsMinute {
                at: 1,
                per_s: 85.5,
                top: vec![sender("10.0.0.7", &["mixer", "this server"], 80.5)],
                over_cap: over,
                browsers: Vec::new(),
            }),
            ..Mdns::default()
        }
    }

    #[test]
    fn the_map_tile_names_the_worst_mdns_trouble_first() {
        assert_eq!(
            tile(&mdns(Vec::new())),
            (
                "quiet".to_owned(),
                "85.5/s last minute · cap 50/s a sender".to_owned(),
                "ok"
            )
        );
        let mut m = mdns(vec![
            sender("10.0.0.7", &["mixer"], 80.5),
            sender("10.0.0.8", &[], 60.0),
        ]);
        if let Some(min) = m.minute.as_mut() {
            min.browsers.push(MdnsBrowser {
                source: "10.0.0.5".into(),
                devices: Vec::new(),
                period_s: 96.0,
            });
        }
        assert_eq!(
            tile(&m),
            (
                "flood".to_owned(),
                "10.0.0.7 (mixer) 80.5/s and 1 more over the cap of 50/s".to_owned(),
                "bad"
            )
        );
        if let Some(min) = m.minute.as_mut() {
            min.over_cap.clear();
        }
        assert_eq!(
            tile(&m),
            (
                "96 s browsing".to_owned(),
                "10.0.0.5 every 96.0 s".to_owned(),
                "bad"
            )
        );
        let off = Mdns {
            running: false,
            error: Some("no Dante address".into()),
            ..Mdns::default()
        };
        assert_eq!(
            tile(&off),
            (
                "not counted".to_owned(),
                "no Dante address".to_owned(),
                "warn"
            )
        );
        let starting = Mdns {
            running: true,
            ..Mdns::default()
        };
        assert_eq!(tile(&starting).2, "ok");
    }

    #[test]
    fn a_sender_reads_with_its_devices_or_its_address_alone() {
        assert_eq!(
            sender_text(&sender("10.0.0.7", &["mixer", "this server"], 80.5)),
            "10.0.0.7 (mixer, this server) 80.5/s"
        );
        assert_eq!(sender_text(&sender("10.0.0.9", &[], 3.0)), "10.0.0.9 3.0/s");
    }

    #[test]
    fn the_alarm_names_every_source_over_the_cap_and_only_then() {
        assert_eq!(alarm(&mdns(Vec::new())), None);
        assert_eq!(alarm(&Mdns::default()), None);
        assert_eq!(
            alarm(&mdns(vec![
                sender("10.0.0.7", &["mixer"], 80.5),
                sender("10.0.0.9", &[], 60.0)
            ]))
            .as_deref(),
            Some(
                "mDNS flood on the Dante leg: 10.0.0.7 (mixer) 80.5/s, 10.0.0.9 60.0/s over the cap of 50/s"
            )
        );
    }

    #[test]
    fn a_browser_reads_with_its_devices_and_cycle_and_alarms_only_when_listed() {
        let mut m = mdns(Vec::new());
        assert_eq!(browser_alarm(&m), None);
        assert_eq!(browser_alarm(&Mdns::default()), None);
        assert!(!browser_summary(&m).1);
        m.minute.as_mut().unwrap().browsers = vec![
            MdnsBrowser {
                source: "10.0.0.7".into(),
                devices: vec!["PC-Studio".into()],
                period_s: 96.4,
            },
            MdnsBrowser {
                source: "10.0.0.9".into(),
                devices: Vec::new(),
                period_s: 96.0,
            },
        ];
        assert_eq!(
            browser_alarm(&m).as_deref(),
            Some(
                "Dante browsed on a 96 s cycle by 10.0.0.7 (PC-Studio) every 96.4 s, 10.0.0.9 every 96.0 s: every Dante host answers it at once"
            )
        );
        assert_eq!(
            browser_summary(&m),
            (
                "96 s browsers: 2 host(s) refresh Dante records every 96 s".to_owned(),
                true
            )
        );
    }

    #[test]
    fn the_floor_alarm_names_each_binary_below_it_and_only_then() {
        let mut m = Mdns {
            floor: "0.6.58".into(),
            ..Mdns::default()
        };
        assert_eq!(floor_alarm(&m), None);
        assert_eq!(
            floor_summary(&m),
            (
                "dstack network floor 0.6.58: no device below it heard in the last hour".to_owned(),
                false
            )
        );
        m.below_floor = vec![
            BelowFloor {
                kind: "peer".into(),
                name: "PC-Studio".into(),
                address: Some("10.0.0.7".into()),
                version: "0.6.20".into(),
            },
            BelowFloor {
                kind: "client".into(),
                name: "dtray".into(),
                address: None,
                version: "0.6.30".into(),
            },
        ];
        assert_eq!(
            floor_alarm(&m).as_deref(),
            Some(
                "dstack below the network floor 0.6.58: PC-Studio at 10.0.0.7 (dstack 0.6.20), dtray (local client, dstack 0.6.30): upgrade it"
            )
        );
        assert!(floor_summary(&m).1);
        m.below_floor[1].address = Some("10.0.0.7".into());
        assert_eq!(
            below_text(&m.below_floor[1]),
            "local client of dtray at 10.0.0.7 (dstack 0.6.30)"
        );
    }

    #[test]
    fn the_summary_warns_on_a_flood_and_on_a_meter_that_does_not_count() {
        let (line, warn) = summary(&mdns(Vec::new()));
        assert_eq!(
            line,
            "mDNS on the Dante leg, last minute: 85.5/s from all senders · cap 50/s a sender"
        );
        assert!(!warn);
        assert!(summary(&mdns(vec![sender("10.0.0.7", &[], 80.0)])).1);
        let first = Mdns {
            running: true,
            ..Mdns::default()
        };
        assert_eq!(
            summary(&first).0,
            "mDNS on the Dante leg: counting, the first minute is not over yet"
        );
        assert!(!summary(&first).1);
        let off = Mdns {
            error: Some("no Dante address".into()),
            ..Mdns::default()
        };
        assert_eq!(
            summary(&off),
            (
                "mDNS on the Dante leg: not counted (no Dante address)".to_owned(),
                true
            )
        );
    }
}
