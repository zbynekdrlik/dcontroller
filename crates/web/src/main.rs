//! dcontroller dashboard: the Dante devices the server discovers, with
//! their channels and subscriptions. Read-only. The version label is
//! build-injected from the same `buildver` as the server's `/api/version`.

use std::collections::BTreeSet;
use std::time::Duration;

use dcontroller_api::{Device, Devices, RxChannel};
use leptos::prelude::*;
use leptos::task::spawn_local;

const VERSION: &str = env!("DC_VERSION");
/// How often the page asks the server for the device list.
const POLL: Duration = Duration::from_secs(3);

async fn fetch_devices() -> Result<Devices, String> {
    let res = gloo_net::http::Request::get("/api/devices")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("server answered {}", res.status()));
    }
    res.json::<Devices>().await.map_err(|e| e.to_string())
}

#[component]
fn App() -> impl IntoView {
    let devices = RwSignal::new(None::<Result<Devices, String>>);
    // The devices whose channels are open, kept across refreshes.
    provide_context(Open(RwSignal::new(BTreeSet::new())));
    let load = move || spawn_local(async move { devices.set(Some(fetch_devices().await)) });
    load();
    // The interval lives as long as the page.
    let _ = set_interval_with_handle(load, POLL);
    view! {
        <header class="bar">
            <h1>"dcontroller"</h1>
            <span class="version" data-testid="version">{VERSION}</span>
        </header>
        <main>
            {move || match devices.get() {
                None => view! { <p class="status" data-testid="status">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! {
                    <p class="status error" data-testid="status">{format!("Server not reachable: {e}")}</p>
                }.into_any(),
                Some(Ok(d)) => view! { <DeviceList d=d/> }.into_any(),
            }}
        </main>
    }
}

/// The status line for a device list.
fn status_line(d: &Devices) -> String {
    let iface = d.interface.as_deref().unwrap_or("-");
    match (d.enabled, &d.error, d.devices.len()) {
        (false, None, _) => "Dante discovery is off: no Dante interface configured.".into(),
        (false, Some(e), _) => format!("Dante discovery is off: {e}"),
        (true, _, 0) => format!("Searching for Dante devices on {iface}…"),
        (true, _, 1) => format!("1 Dante device on {iface}"),
        (true, _, n) => format!("{n} Dante devices on {iface}"),
    }
}

#[component]
fn DeviceList(d: Devices) -> impl IntoView {
    let line = status_line(&d);
    view! {
        <p class="status" data-testid="status">{line}</p>
        <ul class="devices">
            {d.devices.into_iter().map(|dev| view! { <DeviceCard dev=dev/> }).collect_view()}
        </ul>
    }
}

/// The names of the devices whose channels are open.
#[derive(Clone, Copy)]
struct Open(RwSignal<BTreeSet<String>>);

#[component]
fn DeviceCard(dev: Device) -> impl IntoView {
    let Open(open) = expect_context::<Open>();
    let is_open = open.with_untracked(|o| o.contains(&dev.name));
    let key = dev.name.clone();
    let toggle = move |ev: leptos::ev::Event| {
        let now = event_target::<web_sys::HtmlDetailsElement>(&ev).open();
        open.update_untracked(|o| {
            if now {
                o.insert(key.clone());
            } else {
                o.remove(&key);
            }
        });
    };
    let what = [dev.manufacturer.clone(), dev.model.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let counts = dev.channels.as_ref().map_or_else(
        || "channels not read yet".to_string(),
        |c| format!("{} TX · {} RX", c.tx.len(), c.rx.len()),
    );
    let subscribed = dev
        .channels
        .as_ref()
        .map_or(0, |c| c.rx.iter().filter(|r| r.source.is_some()).count());
    let name = dev.name.clone();
    let name_attr = dev.name.clone();
    view! {
        <li class="device" data-testid="device" data-name=name_attr>
            <details open=is_open on:toggle=toggle>
                <summary>
                    <span class="name" data-testid="device-name">{name}</span>
                    <span class="model">{what}</span>
                    <span class="counts" data-testid="device-counts">{counts}</span>
                    <span class="subs">{format!("{subscribed} subscribed")}</span>
                    <span class="addr">{dev.address.clone()}</span>
                </summary>
                {dev.error.clone().map(|e| view! {
                    <p class="error" data-testid="device-error">{format!("Last read failed: {e}")}</p>
                })}
                {dev.channels.map(|c| view! {
                    <h3>"Receive"</h3>
                    <table class="rx" data-testid="rx-table">
                        <thead><tr><th>"#"</th><th>"Channel"</th><th>"Source"</th><th>"Status"</th></tr></thead>
                        <tbody>{c.rx.into_iter().map(|r| view! { <RxRow r=r/> }).collect_view()}</tbody>
                    </table>
                    <h3>"Transmit"</h3>
                    <ol class="tx" data-testid="tx-list">
                        {c.tx.into_iter().map(|t| view! {
                            <li data-testid="tx-channel" value=t.channel>{t.label}</li>
                        }).collect_view()}
                    </ol>
                })}
            </details>
        </li>
    }
}

#[component]
fn RxRow(r: RxChannel) -> impl IntoView {
    let class = match (&r.source, r.receiving()) {
        (None, _) => "status none",
        (Some(_), true) => "status ok",
        (Some(_), false) => "status warn",
    };
    let source = r
        .source
        .as_ref()
        .map_or_else(String::new, |s| format!("{} @ {}", s.channel, s.device));
    view! {
        <tr data-testid="rx-channel">
            <td>{r.channel}</td>
            <td>{r.label.clone()}</td>
            <td data-testid="rx-source">{source}</td>
            <td><span class=class data-testid="rx-status">{r.status.clone().unwrap_or_default()}</span></td>
        </tr>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices(enabled: bool, error: Option<&str>, n: usize) -> Devices {
        Devices {
            enabled,
            interface: Some("eth1".into()),
            error: error.map(Into::into),
            devices: vec![Device::default(); n],
        }
    }

    #[test]
    fn status_line_says_why_discovery_is_off() {
        let mut d = devices(false, None, 0);
        d.interface = None;
        assert_eq!(
            status_line(&d),
            "Dante discovery is off: no Dante interface configured."
        );
        assert_eq!(
            status_line(&devices(
                false,
                Some("interface eth1 has no IPv4 address"),
                0
            )),
            "Dante discovery is off: interface eth1 has no IPv4 address"
        );
    }

    #[test]
    fn status_line_counts_devices() {
        assert_eq!(
            status_line(&devices(true, None, 0)),
            "Searching for Dante devices on eth1…"
        );
        assert_eq!(
            status_line(&devices(true, None, 1)),
            "1 Dante device on eth1"
        );
        assert_eq!(
            status_line(&devices(true, None, 3)),
            "3 Dante devices on eth1"
        );
    }
}
