//! The Devices page's order and "joined" line: a device that just joined
//! the network is found by sorting newest first. Pure, so it is tested on
//! the host.

use dcontroller_api::{Device, Devices};

/// A device first seen within this many seconds of discovery's start was
/// already on the network when dcontroller started.
pub const FIRST_SWEEP_S: u64 = 120;

/// When a device joined, as far as dcontroller knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Joined {
    /// It was there when discovery started.
    AtStart,
    /// It joined at this unix second.
    At(u64),
}

pub fn joined(dev: &Device, d: &Devices) -> Joined {
    if dev.seen_at <= d.started_at + FIRST_SWEEP_S {
        Joined::AtStart
    } else {
        Joined::At(dev.seen_at)
    }
}

/// The devices by name (the server's order), or newest first (latest
/// joined, then by name).
pub fn order(d: &Devices, newest: bool) -> Vec<Device> {
    let mut v = d.devices.clone();
    if newest {
        v.sort_by(|a, b| {
            b.seen_at
                .cmp(&a.seen_at)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d() -> Devices {
        let dev = |name: &str, seen_at: u64| Device {
            name: name.into(),
            seen_at,
            ..Device::default()
        };
        Devices {
            started_at: 1000,
            devices: vec![
                dev("alpha", 1001),
                dev("card", 5000),
                dev("mics", 3000),
                dev("zeta", 1050),
            ],
            ..Devices::default()
        }
    }

    #[test]
    fn newest_first_puts_the_just_joined_device_on_top() {
        let names = |v: Vec<Device>| v.into_iter().map(|x| x.name).collect::<Vec<_>>();
        assert_eq!(names(order(&d(), true)), ["card", "mics", "zeta", "alpha"]);
        assert_eq!(names(order(&d(), false)), ["alpha", "card", "mics", "zeta"]);
    }

    #[test]
    fn a_device_from_the_first_sweep_was_there_at_start() {
        let d = d();
        assert_eq!(joined(&d.devices[0], &d), Joined::AtStart);
        assert_eq!(joined(&d.devices[3], &d), Joined::AtStart);
        assert_eq!(joined(&d.devices[1], &d), Joined::At(5000));
    }
}
