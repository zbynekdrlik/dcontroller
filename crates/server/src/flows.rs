//! A transmitter's TX flows over ARC (dstack `arc::flows`): the list the
//! page shows when an operator opens a device's flows, and the checks and
//! slot choice for a new multicast flow (the write itself runs in
//! `change.rs`, one at a time and logged).

use std::sync::atomic::{AtomicBool, AtomicU16};

use dcontroller_api as api;
use dstack::arc::client::{REPLY_TIMEOUT, Udp};
use dstack::arc::flows::{self, Flows, MAX_FLOW_CHANNELS};
use dstack::ctl::Requests;
use tracing::{info, warn};

use crate::dante::{self, Shared};

/// A flow as the page reads it.
pub fn to_api(f: &flows::TxFlow) -> api::TxFlow {
    api::TxFlow {
        id: f.id,
        slot: f.local_id,
        multicast: f.multicast(),
        channels: f.channels.clone(),
        destinations: f.destinations.iter().map(ToString::to_string).collect(),
    }
}

/// The lowest media-local slot no listed flow holds.
pub fn free_slot(listed: &Flows) -> u16 {
    (1..=u16::MAX)
        .find(|s| !listed.flows.iter().any(|f| f.local_id == *s))
        .unwrap_or(u16::MAX)
}

/// Checks a create before anything is sent: 1 to 8 channels, each from 1,
/// none twice, each one the device transmits (`tx` its TX channel numbers).
pub fn validate(req: &api::MulticastCreate, tx: &[u16]) -> Result<(), String> {
    let c = &req.channels;
    if c.is_empty() {
        return Err("choose at least one channel".into());
    }
    if c.len() > MAX_FLOW_CHANNELS {
        return Err(format!(
            "a flow carries at most {MAX_FLOW_CHANNELS} channels"
        ));
    }
    if let Some(ch) = c.iter().find(|ch| !tx.contains(ch)) {
        return Err(format!("{} has no TX channel {ch}", req.device));
    }
    if (1..c.len()).any(|i| c[..i].contains(&c[i])) {
        return Err("a channel is chosen twice".into());
    }
    Ok(())
}

/// The flow text the log keeps: `slot 3 multicast 1,2 -> 239.255.1.2:4321`.
pub fn text(f: &flows::TxFlow) -> String {
    let ch = f
        .channels
        .iter()
        // 0: an empty channel slot of the flow.
        .filter(|&&n| n != 0)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let to = f
        .destinations
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let kind = if f.multicast() {
        "multicast"
    } else {
        "unicast"
    };
    if to.is_empty() {
        format!("slot {} {kind} {ch}", f.local_id)
    } else {
        format!("slot {} {kind} {ch} -> {to}", f.local_id)
    }
}

/// Lists `device`'s TX flows (one ARC request).
pub fn list(dante: &Shared, device: &str) -> Result<api::TxFlows, String> {
    let target = dante::State::target(&dante.read().unwrap_or_else(|e| e.into_inner()), device);
    let Some((ip, dev)) = target else {
        return Err(format!(
            "{device} is not listed (discovery off or device gone)"
        ));
    };
    let seq = AtomicU16::new(1);
    let requests = Requests::default();
    let stop = AtomicBool::new(false);
    let mut udp = Udp::open(ip, dev.arc, &seq, REPLY_TIMEOUT, &requests, &stop)
        .map_err(|e| format!("socket: {e}"))?;
    match flows::list_tx_flows(&mut udp) {
        Ok(f) => {
            info!(device = %dev.name, flows = f.flows.len(), unread = f.unread, "flows: listed");
            Ok(api::TxFlows {
                device: dev.name.clone(),
                flows: f.flows.iter().map(to_api).collect(),
                unread: f.unread,
            })
        }
        Err(e) => {
            warn!(device = %dev.name, error = %e, "flows: list failed");
            Err(format!("{}: {e}", dev.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(slot: u16, channels: &[u16], multicast: bool) -> flows::TxFlow {
        flows::TxFlow {
            id: slot + 100,
            local_id: slot,
            flags: if multicast {
                flows::MULTICAST
            } else {
                flows::UNICAST
            },
            channels: channels.to_vec(),
            fpp: 16,
            bits: 24,
            destinations: vec!["239.255.1.2:4321".parse().unwrap()],
        }
    }

    #[test]
    fn the_free_slot_is_the_lowest_no_flow_holds() {
        let none = Flows::default();
        assert_eq!(free_slot(&none), 1);
        let held = Flows {
            flows: vec![
                flow(1, &[1], false),
                flow(2, &[2], true),
                flow(4, &[3], false),
            ],
            unread: 0,
        };
        assert_eq!(free_slot(&held), 3);
    }

    #[test]
    fn a_create_names_one_to_eight_channels_the_device_has_once() {
        let tx: Vec<u16> = (1..=16).collect();
        let r = |c: &[u16]| api::MulticastCreate {
            device: "mics".into(),
            channels: c.to_vec(),
        };
        assert!(validate(&r(&[1, 2]), &tx).is_ok());
        assert!(validate(&r(&[]), &tx).unwrap_err().contains("at least one"));
        assert!(
            validate(&r(&[1, 2, 3, 4, 5, 6, 7, 8, 9]), &tx)
                .unwrap_err()
                .contains("at most 8")
        );
        assert!(
            validate(&r(&[0]), &tx)
                .unwrap_err()
                .contains("no TX channel 0")
        );
        assert!(
            validate(&r(&[17]), &tx)
                .unwrap_err()
                .contains("no TX channel 17")
        );
        assert!(validate(&r(&[3, 3]), &tx).unwrap_err().contains("twice"));
    }

    #[test]
    fn a_flow_reads_as_slot_kind_channels_and_group() {
        assert_eq!(
            text(&flow(3, &[1, 2], true)),
            "slot 3 multicast 1,2 -> 239.255.1.2:4321"
        );
        let mut f = flow(2, &[5, 0, 0, 0], false);
        f.destinations.clear();
        assert_eq!(text(&f), "slot 2 unicast 5");
        let a = to_api(&flow(3, &[1, 2], true));
        assert!(a.multicast);
        assert_eq!((a.slot, a.id), (3, 103));
        assert_eq!(a.destinations, vec!["239.255.1.2:4321".to_string()]);
    }
}
