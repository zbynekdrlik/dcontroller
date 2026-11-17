//! E2E helper (dcontroller #108): a real dstack Dante device that holds
//! bridge figures, as a dasio bridge instance does, so the E2E reads them
//! over the network the way dcontroller reads a dasio bridge. The figures
//! are fixed except the to-ASIO underruns, which tick every 10 s (as
//! dasio's hand-over does) with the cause `driver`.
//!
//! Usage: bridge_device <config.toml> <ip> <control socket>. Runs until
//! SIGTERM; exits non-zero when the device does not start or fails.

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use dstack::daemon::{Options, Place};
use dstack::host::{self, ClockSource, Signals};
use dstack::info::{OwnFigures, UnderrunCause};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [config, ip, control] = args.as_slice() else {
        anyhow::bail!("usage: bridge_device <config.toml> <ip> <control socket>");
    };
    let ip: Ipv4Addr = ip.parse()?;
    let opts = Options {
        config: PathBuf::from(config),
        place: Place::Ip(ip),
        control: PathBuf::from(control),
    };
    let prepared = host::prepare_with(&opts, &[], ClockSource::System)
        .map_err(|e| anyhow::anyhow!("device not set up: {e}"))?;
    let media = prepared.media();
    media.bridge_figures().set_source(|| OwnFigures {
        to_asio_target_us: 1000,
        from_asio_target_us: 1667,
        to_asio_converter_delay_us: 167,
        from_asio_converter_delay_us: 167,
        late_callbacks: 2,
        late_bins: [40, 12, 5, 2, 1, 0, 0, 1],
    });
    let slot = media.bridge_figures().clone();
    let stop = Arc::new(AtomicBool::new(false));
    let ticks = Arc::new(AtomicU64::new(0));
    {
        let (stop, ticks) = (Arc::clone(&stop), Arc::clone(&ticks));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let n = ticks.fetch_add(1, Ordering::Relaxed) + 1;
                slot.set_underruns(n, 0, UnderrunCause::Driver, UnderrunCause::None);
                std::thread::sleep(Duration::from_secs(10));
            }
        });
    }
    let running = prepared
        .start(Signals::Stop)
        .map_err(|e| anyhow::anyhow!("device not started: {e}"))?;
    println!("bridge_device: serving on {ip}");
    let r = running.wait();
    stop.store(true, Ordering::Relaxed);
    r.map_err(|e| anyhow::anyhow!("device failed: {e}"))
}
