//! The clock view's rows: each device's PTP role and sync, its master by
//! name where the master is a listed device, and what needs a look. Pure,
//! so it is tested on the host.

use dcontroller_api::{Clock, Devices};

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

#[cfg(test)]
mod tests {
    use super::*;
    use dcontroller_api::Device;

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
