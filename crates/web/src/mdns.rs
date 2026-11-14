//! The mDNS senders on the Dante leg (dcontroller #86, dstack #289): the
//! rates on the Network page and the alarm on every page while a source
//! sends over the cap.

use dcontroller_api::{Mdns, MdnsSender};

/// A sender as `address (devices) n/s`.
pub fn sender_text(s: &MdnsSender) -> String {
    let who = if s.devices.is_empty() {
        s.source.clone()
    } else {
        format!("{} ({})", s.source, s.devices.join(", "))
    };
    format!("{who} {}/s", s.per_s)
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

/// The Network page's line on the mDNS meter, and whether it warns.
pub fn summary(m: &Mdns) -> (String, bool) {
    match (&m.minute, m.running) {
        (Some(min), _) => (
            format!(
                "mDNS on the Dante leg, last minute: {}/s from all senders · cap {}/s a sender",
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
            }),
        }
    }

    #[test]
    fn a_sender_reads_with_its_devices_or_its_address_alone() {
        assert_eq!(
            sender_text(&sender("10.0.0.7", &["mixer", "this server"], 80.5)),
            "10.0.0.7 (mixer, this server) 80.5/s"
        );
        assert_eq!(sender_text(&sender("10.0.0.9", &[], 3.0)), "10.0.0.9 3/s");
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
                "mDNS flood on the Dante leg: 10.0.0.7 (mixer) 80.5/s, 10.0.0.9 60/s over the cap of 50/s"
            )
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
