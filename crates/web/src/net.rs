//! The network view's rows: each device's primary and secondary interface,
//! and which links run below a gigabit. Pure, so it is tested on the host.

use dcontroller_api::{Devices, NetPort};

/// One device's row in the network view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub device: String,
    /// `None`: no network info (`error` says why, when read).
    pub primary: Option<NetPort>,
    pub secondary: Option<NetPort>,
    pub error: Option<String>,
    /// The primary link is down or below 1 Gbit/s.
    pub warn: bool,
}

/// A link speed as Dante Controller writes it.
pub fn link(mbit: u16) -> String {
    match mbit {
        0 => "no link".to_owned(),
        m if m % 1000 == 0 => format!("{} Gbit/s", m / 1000),
        m => format!("{m} Mbit/s"),
    }
}

/// The rows, in the server's order.
pub fn rows(d: &Devices) -> Vec<Row> {
    d.devices
        .iter()
        .map(|dev| {
            let ports = dev.network.clone().unwrap_or_default();
            let primary = ports.first().cloned();
            Row {
                device: dev.name.clone(),
                warn: primary.as_ref().is_some_and(|p| p.link_mbit < 1000),
                primary,
                secondary: ports.get(1).cloned(),
                error: dev.network_error.clone(),
            }
        })
        .collect()
}

/// The summary line, and whether it warns.
pub fn summary(rows: &[Row]) -> (String, bool) {
    let read = rows.iter().filter(|r| r.primary.is_some()).count();
    let slow: Vec<&str> = rows
        .iter()
        .filter(|r| r.warn)
        .map(|r| r.device.as_str())
        .collect();
    if slow.is_empty() {
        (
            format!("{read} devices report their network · all at 1 Gbit/s or more"),
            false,
        )
    } else {
        (
            format!(
                "{read} devices report their network · below 1 Gbit/s: {}",
                slow.join(", ")
            ),
            true,
        )
    }
}

/// A port as one short text: link and address.
pub fn port_text(p: &NetPort) -> String {
    if p.link_mbit == 0 {
        link(0)
    } else {
        format!("{} · {}", link(p.link_mbit), p.ip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcontroller_api::Device;

    fn port(mbit: u16, ip: &str) -> NetPort {
        NetPort {
            link_mbit: mbit,
            ip: ip.into(),
            ..NetPort::default()
        }
    }

    fn dev(name: &str, network: Option<Vec<NetPort>>, error: Option<&str>) -> Device {
        Device {
            name: name.into(),
            network,
            network_error: error.map(str::to_owned),
            ..Device::default()
        }
    }

    fn site() -> Devices {
        Devices {
            devices: vec![
                dev(
                    "card",
                    Some(vec![port(1000, "10.0.0.2"), port(0, "0.0.0.0")]),
                    None,
                ),
                dev("mic", Some(vec![port(100, "10.0.0.3")]), None),
                dev("dvs", None, Some("no reply")),
            ],
            ..Devices::default()
        }
    }

    #[test]
    fn link_speeds_read_as_dante_controller_writes_them() {
        assert_eq!(link(0), "no link");
        assert_eq!(link(100), "100 Mbit/s");
        assert_eq!(link(1000), "1 Gbit/s");
        assert_eq!(link(10000), "10 Gbit/s");
        assert_eq!(port_text(&port(1000, "10.0.0.2")), "1 Gbit/s · 10.0.0.2");
        assert_eq!(port_text(&port(0, "0.0.0.0")), "no link");
    }

    #[test]
    fn rows_split_primary_and_secondary_and_flag_a_slow_primary() {
        let r = rows(&site());
        assert_eq!(r[0].primary.as_ref().map(|p| p.link_mbit), Some(1000));
        assert_eq!(r[0].secondary.as_ref().map(|p| p.link_mbit), Some(0));
        assert!(!r[0].warn);
        assert_eq!(r[1].secondary, None);
        assert!(r[1].warn);
        assert_eq!(r[2].primary, None);
        assert_eq!(r[2].error.as_deref(), Some("no reply"));
        assert!(!r[2].warn);
        assert_eq!(
            summary(&r),
            (
                "2 devices report their network · below 1 Gbit/s: mic".to_owned(),
                true
            )
        );
        assert_eq!(
            summary(&r[..1]),
            (
                "1 devices report their network · all at 1 Gbit/s or more".to_owned(),
                false
            )
        );
    }
}
