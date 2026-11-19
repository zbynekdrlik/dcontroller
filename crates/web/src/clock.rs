//! The clock view's rows: each device's PTP role and sync, its master by
//! name where the master is a listed device, and what needs a look. Pure,
//! so it is tested on the host.

use dcontroller_api::{Clock, ClockNode, Clocks, Devices};

/// One device's row in the clock view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub device: String,
    /// `None`: no clock status (`error` says why, when read).
    pub clock: Option<Clock>,
    pub error: Option<String>,
    /// The master's device name, else its clock id; `None` for a leader.
    pub master: Option<String>,
    /// A follower that is not synchronized.
    pub warn: bool,
}

/// The role as Dante Controller names it.
pub fn role(c: &Clock) -> &'static str {
    match (c.leader, c.grandmaster) {
        (true, true) => "Leader (grandmaster)",
        (true, false) => "Leader",
        (false, _) => "Follower",
    }
}

/// The rows, in the server's order.
pub fn rows(d: &Devices) -> Vec<Row> {
    let name_of = |id: &str| {
        d.devices
            .iter()
            .find(|x| x.clock.as_ref().is_some_and(|c| c.id == id))
            .map(|x| x.name.clone())
    };
    d.devices
        .iter()
        .map(|dev| {
            let master = dev
                .clock
                .as_ref()
                .filter(|c| !c.leader)
                .map(|c| name_of(&c.master).unwrap_or_else(|| c.master.clone()));
            Row {
                device: dev.name.clone(),
                clock: dev.clock.clone(),
                error: dev.clock_error.clone(),
                master,
                warn: dev
                    .clock
                    .as_ref()
                    .is_some_and(|c| !c.leader && c.sync != "synchronized"),
            }
        })
        .collect()
}

/// The names of the leading devices.
pub fn leaders(rows: &[Row]) -> Vec<&str> {
    rows.iter()
        .filter(|r| r.clock.as_ref().is_some_and(|c| c.leader))
        .map(|r| r.device.as_str())
        .collect()
}

/// The summary line: who leads, and a warning for none or several.
pub fn summary(rows: &[Row]) -> (String, bool) {
    let l = leaders(rows);
    let followers = rows
        .iter()
        .filter(|r| r.clock.as_ref().is_some_and(|c| !c.leader))
        .count();
    let off = rows.iter().filter(|r| r.warn).count();
    match l.as_slice() {
        [] => ("No clock leader answers".to_owned(), true),
        [one] => (
            format!(
                "Leader: {one} · {followers} following{}",
                if off > 0 {
                    format!(" · {off} not synchronized")
                } else {
                    String::new()
                }
            ),
            off > 0,
        ),
        many => (format!("Several leaders: {}", many.join(", ")), true),
    }
}

/// The frequency offset as a signed ppb figure.
pub fn ppb(v: i32) -> String {
    format!("{v:+} ppb")
}

/// A time error for reading: `0.85 µs`, `12.3 µs`, `1.20 ms`.
pub fn ns_text(ns: i64) -> String {
    let abs = ns.unsigned_abs();
    let sign = if ns < 0 { "-" } else { "" };
    if abs < 10_000 {
        format!("{sign}{:.2} µs", abs as f64 / 1e3)
    } else if abs < 1_000_000 {
        format!("{sign}{:.1} µs", abs as f64 / 1e3)
    } else {
        format!("{sign}{:.2} ms", abs as f64 / 1e6)
    }
}

/// The dsync nodes with the troubled ones on top (bad, then limping, then
/// good), each group by name.
pub fn dsync_order(c: &Clocks) -> Vec<&ClockNode> {
    let rank = |n: &ClockNode| match n.health().0 {
        "bad" => 0,
        "warn" => 1,
        _ => 2,
    };
    let mut v: Vec<&ClockNode> = c.nodes.iter().collect();
    v.sort_by_key(|n| (rank(n), n.device.to_lowercase()));
    v
}

/// The listed devices that gave no clock figures, by name.
pub fn dsync_silent(d: &Devices, c: &Clocks) -> Vec<String> {
    d.devices
        .iter()
        .filter(|dev| {
            !c.nodes
                .iter()
                .any(|n| n.device.eq_ignore_ascii_case(&dev.name))
        })
        .map(|dev| dev.name.clone())
        .collect()
}

/// One line for the dsync part: how many are locked, who is not.
pub fn dsync_summary(c: &Clocks) -> (String, bool) {
    let bad: Vec<&str> = c
        .nodes
        .iter()
        .filter(|n| n.health().0 == "bad")
        .map(|n| n.device.as_str())
        .collect();
    let limping = c.nodes.iter().filter(|n| n.health().0 == "warn").count();
    let good = c.nodes.len() - bad.len() - limping;
    if c.nodes.is_empty() {
        return ("no device gives dsync clock figures".to_owned(), false);
    }
    let mut line = format!("dsync: {good} of {} locked", c.nodes.len());
    if limping > 0 {
        line.push_str(&format!(" · {limping} limping"));
    }
    if !bad.is_empty() {
        line.push_str(&format!(" · no usable time: {}", bad.join(", ")));
    }
    (line, !bad.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcontroller_api::Device;

    #[test]
    fn dsync_nodes_read_troubles_first_and_name_the_silent_devices() {
        use dcontroller_api::Follower;
        let node = |name: &str, locked: bool, holdover: bool| ClockNode {
            device: name.into(),
            ready: true,
            source: "follower".into(),
            follower: Some(Follower {
                locked,
                settled: locked,
                holdover,
                ..Follower::default()
            }),
            ..ClockNode::default()
        };
        let c = Clocks {
            now: 0,
            nodes: vec![
                node("b-good", true, false),
                node("a-good", true, false),
                node("limp", true, true),
                node("lost", false, false),
            ],
        };
        let order: Vec<&str> = dsync_order(&c).iter().map(|n| n.device.as_str()).collect();
        assert_eq!(order, ["lost", "limp", "a-good", "b-good"]);
        assert_eq!(
            dsync_summary(&c),
            (
                "dsync: 2 of 4 locked · 1 limping · no usable time: lost".to_owned(),
                true
            )
        );
        assert!(!dsync_summary(&Clocks::default()).1);
        let d = Devices {
            devices: vec![
                Device {
                    name: "LOST".into(),
                    ..Device::default()
                },
                Device {
                    name: "amp".into(),
                    ..Device::default()
                },
            ],
            ..Devices::default()
        };
        assert_eq!(dsync_silent(&d, &c), ["amp"]);
        assert_eq!(ns_text(850), "0.85 µs");
        assert_eq!(ns_text(-12_300), "-12.3 µs");
        assert_eq!(ns_text(1_200_000), "1.20 ms");
    }

    fn clock(id: &str, master: &str, leader: bool, sync: &str) -> Clock {
        Clock {
            leader,
            grandmaster: leader,
            state: "disciplined".into(),
            sync: sync.into(),
            id: id.into(),
            master: master.into(),
            grandmaster_id: master.into(),
            ports: vec![if leader { "leader" } else { "follower" }.into()],
            ..Clock::default()
        }
    }

    fn dev(name: &str, clock: Option<Clock>, error: Option<&str>) -> Device {
        Device {
            name: name.into(),
            clock,
            clock_error: error.map(str::to_owned),
            ..Device::default()
        }
    }

    fn site() -> Devices {
        Devices {
            devices: vec![
                dev("card", Some(clock("aa", "aa", true, "none")), None),
                dev("desk", Some(clock("bb", "aa", false, "synchronized")), None),
                dev("mic", Some(clock("cc", "zz", false, "servo 5")), None),
                dev("soft", None, Some("no reply")),
            ],
            ..Devices::default()
        }
    }

    #[test]
    fn rows_name_the_master_and_flag_an_unsynchronized_follower() {
        let r = rows(&site());
        assert_eq!(r[0].master, None);
        assert!(!r[0].warn);
        assert_eq!(r[1].master.as_deref(), Some("card"));
        assert!(!r[1].warn);
        // A master that is no listed device shows its clock id.
        assert_eq!(r[2].master.as_deref(), Some("zz"));
        assert!(r[2].warn);
        assert_eq!(r[3].clock, None);
        assert_eq!(r[3].error.as_deref(), Some("no reply"));
        assert!(!r[3].warn);
        assert_eq!(role(r[0].clock.as_ref().unwrap()), "Leader (grandmaster)");
        assert_eq!(role(r[1].clock.as_ref().unwrap()), "Follower");
        let mut c = clock("x", "y", true, "none");
        c.grandmaster = false;
        assert_eq!(role(&c), "Leader");
    }

    #[test]
    fn summary_names_the_leader_and_warns_on_none_several_or_drift() {
        let r = rows(&site());
        assert_eq!(
            summary(&r),
            (
                "Leader: card · 2 following · 1 not synchronized".to_owned(),
                true
            )
        );
        assert_eq!(
            summary(&r[..2]),
            ("Leader: card · 1 following".to_owned(), false)
        );
        assert_eq!(
            summary(&r[1..]),
            ("No clock leader answers".to_owned(), true)
        );
        let mut two = r.clone();
        two[1].clock.as_mut().unwrap().leader = true;
        assert_eq!(
            summary(&two),
            ("Several leaders: card, desk".to_owned(), true)
        );
        assert_eq!(ppb(-1200), "-1200 ppb");
        assert_eq!(ppb(35), "+35 ppb");
    }
}
