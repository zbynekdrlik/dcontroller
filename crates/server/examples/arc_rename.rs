//! E2E helper: renames a channel of a device the way another controller
//! would (Dante Controller, a mixer), straight over ARC through dstack, not
//! through dcontroller. dstack removed its `dstack-dev card` tool (v0.6.95);
//! `dstack::arc::control` is its replacement.
//!
//! Usage: arc_rename <our ip> <device ARC ip:port> rx|tx <channel> <label|->
//! (`-` resets the label to the device's default). Prints the label read
//! back; exits 1 on any error.

use std::sync::atomic::{AtomicBool, AtomicU16};

use dstack::arc::client::{REPLY_TIMEOUT, Udp};
use dstack::arc::control::{self, Dir};
use dstack::ctl::Requests;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [ip, arc, dir, channel, label] = args.as_slice() else {
        anyhow::bail!("usage: arc_rename <our ip> <device ARC ip:port> rx|tx <channel> <label|->");
    };
    let dir = match dir.as_str() {
        "rx" => Dir::Rx,
        "tx" => Dir::Tx,
        other => anyhow::bail!("rx or tx, not {other:?}"),
    };
    let channel: u16 = channel.parse()?;
    let label = (label != "-").then_some(label.as_str());
    let (seq, requests, stop) = (AtomicU16::new(1), Requests::default(), AtomicBool::new(false));
    let mut udp = Udp::open(ip.parse()?, arc.parse()?, &seq, REPLY_TIMEOUT, &requests, &stop)?;
    control::rename_channel(&mut udp, dir, channel, label)?;
    let now = control::labels(&mut udp)?;
    let read = now
        .get(dir, channel)
        .ok_or_else(|| anyhow::anyhow!("no channel {channel}"))?;
    println!("{read}");
    Ok(())
}
