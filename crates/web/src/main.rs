//! dcontroller dashboard: the Dante devices the server discovers, with
//! their channels and subscriptions, and the routing matrix. A click on a
//! cross-point asks for a subscription change; nothing is sent before the
//! user confirms it, and the server reads it back and logs it. The version label is
//! build-injected from the same `buildver` as the server's `/api/version`.

use std::collections::BTreeSet;
use std::time::Duration;

use dcontroller_api::{
    ChangeEntry, ChangeResult, Device, Devices, RxChannel, Source, SubscriptionChange,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

mod matrix;
use matrix::{Grid, cell, elsewhere};

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
    provide_context(Expanded {
        tx: RwSignal::new(BTreeSet::new()),
        rx: RwSignal::new(BTreeSet::new()),
    });
    let view_now = RwSignal::new(View::from_hash());
    let ctl = Ctl {
        devices,
        pending: RwSignal::new(None),
        busy: RwSignal::new(false),
        result: RwSignal::new(None),
    };
    provide_context(ctl);
    let load = move || spawn_local(async move { devices.set(Some(fetch_devices().await)) });
    load();
    // The interval lives as long as the page.
    let _ = set_interval_with_handle(load, POLL);
    view! {
        <header class="bar">
            <h1>"dcontroller"</h1>
            <nav class="tabs">
                <a href="#routing" data-testid="tab-routing"
                    class=move || if view_now.get() == View::Routing { "tab on" } else { "tab" }
                    on:click=move |_| view_now.set(View::Routing)>"Routing"</a>
                <a href="#devices" data-testid="tab-devices"
                    class=move || if view_now.get() == View::Devices { "tab on" } else { "tab" }
                    on:click=move |_| view_now.set(View::Devices)>"Devices"</a>
                <a href="#changes" data-testid="tab-changes"
                    class=move || if view_now.get() == View::Changes { "tab on" } else { "tab" }
                    on:click=move |_| view_now.set(View::Changes)>"Changes"</a>
            </nav>
            <span class="version" data-testid="version">{VERSION}</span>
        </header>
        <main>
            {move || match devices.get() {
                None => view! { <p class="status" data-testid="status">"Loading…"</p> }.into_any(),
                Some(Err(e)) => view! {
                    <p class="status error" data-testid="status">{format!("Server not reachable: {e}")}</p>
                }.into_any(),
                Some(Ok(d)) => match view_now.get() {
                    View::Devices => view! { <DeviceList d=d/> }.into_any(),
                    View::Routing => view! { <Routing d=d/> }.into_any(),
                    View::Changes => view! { <ChangeLog/> }.into_any(),
                },
            }}
        </main>
        <ConfirmDialog/>
        <ResultLine/>
    }
}

/// The page's view, kept in the URL hash.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Routing,
    Devices,
    Changes,
}

impl View {
    fn from_hash() -> View {
        let hash = window().location().hash().unwrap_or_default();
        match hash.as_str() {
            "#devices" => View::Devices,
            "#changes" => View::Changes,
            _ => View::Routing,
        }
    }
}

/// A subscription change the user clicked, waiting for the confirmation.
#[derive(Clone, Debug, PartialEq)]
struct Pending {
    rx_device: String,
    rx_channel: u16,
    rx_label: String,
    current: Option<Source>,
    /// `None` = remove the subscription.
    target: Option<Source>,
}

/// What a click on the cross-point of `rc` and TX channel `tx_channel` of
/// `tx_device` asks for: the subscription to it, or its removal when the
/// channel already takes exactly it.
fn pending_for(rx_device: &str, rc: &RxChannel, tx_device: &str, tx_channel: &str) -> Pending {
    let hit = rc.source.as_ref().is_some_and(|s| {
        s.device.eq_ignore_ascii_case(tx_device) && s.channel.eq_ignore_ascii_case(tx_channel)
    });
    Pending {
        rx_device: rx_device.to_owned(),
        rx_channel: rc.channel,
        rx_label: rc.label.clone(),
        current: rc.source.clone(),
        target: (!hit).then(|| Source {
            channel: tx_channel.to_owned(),
            device: tx_device.to_owned(),
        }),
    }
}

fn source_text(s: &Option<Source>) -> String {
    s.as_ref().map_or_else(
        || "nothing".to_owned(),
        |s| format!("{} @ {}", s.channel, s.device),
    )
}

/// The confirmation question for `p`.
fn question(p: &Pending) -> String {
    let rx = format!("{} @ {}", p.rx_label, p.rx_device);
    match (&p.current, &p.target) {
        (_, None) => format!(
            "Remove the subscription of {rx} (now {})?",
            source_text(&p.current)
        ),
        (None, Some(_)) => format!("Subscribe {rx} to {}?", source_text(&p.target)),
        (Some(_), Some(_)) => format!(
            "Change {rx} from {} to {}?",
            source_text(&p.current),
            source_text(&p.target)
        ),
    }
}

/// The page's change state: the device list, the change waiting for its
/// confirmation, one change in flight, and the last result.
#[derive(Clone, Copy)]
struct Ctl {
    devices: RwSignal<Option<Result<Devices, String>>>,
    pending: RwSignal<Option<Pending>>,
    busy: RwSignal<bool>,
    result: RwSignal<Option<ChangeResult>>,
}

async fn send_change(req: &SubscriptionChange) -> Result<ChangeResult, String> {
    let res = gloo_net::http::Request::post("/api/subscription")
        .json(req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("server answered {}", res.status()));
    }
    res.json::<ChangeResult>().await.map_err(|e| e.to_string())
}

#[component]
fn ConfirmDialog() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let confirm = move |_| {
        let Some(p) = ctl.pending.get_untracked() else {
            return;
        };
        ctl.busy.set(true);
        spawn_local(async move {
            let req = SubscriptionChange {
                rx_device: p.rx_device.clone(),
                rx_channel: p.rx_channel,
                source: p.target.clone(),
            };
            let r = send_change(&req).await.unwrap_or_else(|e| ChangeResult {
                ok: false,
                message: e,
                channel: None,
            });
            ctl.result.set(Some(r));
            ctl.devices.set(Some(fetch_devices().await));
            ctl.busy.set(false);
            ctl.pending.set(None);
        });
    };
    move || {
        ctl.pending.get().map(|p| {
            let busy = ctl.busy.get();
            view! {
                <div class="modal-back">
                    <div class="modal" role="dialog" data-testid="confirm-dialog">
                        <p data-testid="confirm-text">{question(&p)}</p>
                        <div class="buttons">
                            <button data-testid="cancel-change" disabled=busy
                                on:click=move |_| ctl.pending.set(None)>"Cancel"</button>
                            <button class="primary" data-testid="confirm-change" disabled=busy
                                on:click=confirm>{if busy { "Sending…" } else { "Confirm" }}</button>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

#[component]
fn ResultLine() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    move || {
        ctl.result.get().map(|r| {
            let class = if r.ok { "result ok" } else { "result err" };
            view! {
                <div class=class data-testid="change-result" on:click=move |_| ctl.result.set(None)>
                    {if r.ok { format!("Done: {}", r.message) } else { format!("Not changed: {}", r.message) }}
                </div>
            }
        })
    }
}

async fn fetch_changes() -> Result<Vec<ChangeEntry>, String> {
    let res = gloo_net::http::Request::get("/api/changes")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("server answered {}", res.status()));
    }
    res.json::<Vec<ChangeEntry>>()
        .await
        .map_err(|e| e.to_string())
}

/// The changes log, newest first.
#[component]
fn ChangeLog() -> impl IntoView {
    let list = RwSignal::new(None::<Result<Vec<ChangeEntry>, String>>);
    spawn_local(async move { list.set(Some(fetch_changes().await)) });
    move || {
        match list.get() {
        None => view! { <p class="status">"Loading…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="status error">{format!("Changes not readable: {e}")}</p> }.into_any(),
        Some(Ok(v)) if v.is_empty() => view! { <p class="status" data-testid="changes-empty">"No changes yet."</p> }.into_any(),
        Some(Ok(v)) => view! {
            <table class="changes">
                <thead><tr><th>"When"</th><th>"Who"</th><th>"Channel"</th><th>"Before"</th><th>"Asked"</th><th>"After"</th><th>"Result"</th></tr></thead>
                <tbody>
                    {v.into_iter().map(|e| {
                        let when = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(e.at as f64 * 1000.0))
                            .to_locale_string("sk-SK", &wasm_bindgen::JsValue::UNDEFINED);
                        view! {
                            <tr data-testid="change-row" class=if e.ok { "" } else { "err" }>
                                <td>{String::from(when)}</td>
                                <td>{e.user}</td>
                                <td>{format!("{} @ {}", e.rx_label, e.rx_device)}</td>
                                <td>{source_text(&e.before)}</td>
                                <td>{source_text(&e.requested)}</td>
                                <td>{source_text(&e.after)}</td>
                                <td>{e.message}</td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        }.into_any(),
    }
    }
}

/// The expanded TX and RX devices of the matrix, kept across refreshes.
#[derive(Clone, Copy)]
struct Expanded {
    tx: RwSignal<BTreeSet<String>>,
    rx: RwSignal<BTreeSet<String>>,
}

fn flip(set: RwSignal<BTreeSet<String>>, name: &str) {
    set.update(|s| {
        if !s.remove(name) {
            s.insert(name.to_owned());
        }
    });
}

/// The routing matrix: TX devices as columns, RX devices as rows, each
/// device collapsed to one cross-point until it is clicked open.
#[component]
fn Routing(d: Devices) -> impl IntoView {
    let ex = expect_context::<Expanded>();
    // Re-rendered when a device is opened or closed, not only on a refresh.
    move || routing_view(&d, ex)
}

fn routing_view(d: &Devices, ex: Expanded) -> AnyView {
    let line = status_line(d);
    let grid = Grid::of(d);
    let Expanded {
        tx: tx_open,
        rx: rx_open,
    } = ex;
    let ctl = expect_context::<Ctl>();
    let txo = tx_open.get();
    let rxo = rx_open.get();
    // Column plan: (device index, None) for a collapsed device, (device, Some(channel)) per open channel.
    let cols: Vec<(usize, Option<usize>)> = grid
        .tx
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            if txo.contains(&t.device) {
                (0..t.channels.len())
                    .map(|c| (i, Some(c)))
                    .collect::<Vec<_>>()
            } else {
                vec![(i, None)]
            }
        })
        .collect();
    let head_devices = grid
        .tx
        .iter()
        .map(|t| {
            let open = txo.contains(&t.device);
            let span = if open { t.channels.len() } else { 1 };
            let name = t.device.clone();
            let label = t.device.clone();
            let name_attr = t.device.clone();
            view! {
                <th class=if open { "txdev open" } else { "txdev" } colspan=span
                    data-testid="tx-device" data-name=name_attr
                    on:click=move |_| flip(tx_open, &name)>
                    <span>{label}</span>
                </th>
            }
        })
        .collect_view();
    let head_channels = cols
        .iter()
        .map(|&(i, c)| match c {
            Some(c) => {
                view! { <th class="txch"><span>{grid.tx[i].channels[c].clone()}</span></th> }
                    .into_any()
            }
            None => view! { <th class="txch"></th> }.into_any(),
        })
        .collect_view();
    let mut rows = Vec::new();
    for r in &grid.rx {
        let open = rxo.contains(&r.device);
        let name = r.device.clone();
        let label = r.device.clone();
        let name_attr = r.device.clone();
        let cells = cols
            .iter()
            .map(|&(i, c)| {
                let t = &grid.tx[i];
                let ch = c.map(|c| t.channels[c].as_str());
                let x = cell(&r.channels, &t.device, ch);
                let n = x.count();
                let tx_attr = match ch {
                    Some(ch) => format!("{}/{}", t.device, ch),
                    None => t.device.clone(),
                };
                view! {
                    <td class=x.class() data-testid="cell" data-rx=r.device.clone() data-tx=tx_attr>
                        {(n > 0).then(|| n.to_string())}
                    </td>
                }
            })
            .collect_view();
        rows.push(view! {
            <tr class="rxdev">
                <th class=if open { "rxname open" } else { "rxname" } data-testid="rx-device" data-name=name_attr
                    on:click=move |_| flip(rx_open, &name)>{label}</th>
                {cells}
            </tr>
        }.into_any());
        if open {
            for rc in &r.channels {
                let cells = cols
                    .iter()
                    .map(|&(i, c)| {
                        let t = &grid.tx[i];
                        let ch = c.map(|c| t.channels[c].as_str());
                        let x = cell(std::slice::from_ref(rc), &t.device, ch);
                        let tx_attr = match ch {
                            Some(ch) => format!("{}/{}", t.device, ch),
                            None => t.device.clone(),
                        };
                        // A collapsed TX device shows which of its channels feeds this one.
                        let text = match (ch, &rc.source) {
                            (None, Some(s)) if x.count() > 0 => s.channel.clone(),
                            (Some(_), _) if x.count() > 0 => "●".into(),
                            _ => String::new(),
                        };
                        // Channel by channel, a click asks for a change.
                        let ask = ch.map(|ch| pending_for(&r.device, rc, &t.device, ch));
                        let class = if ask.is_some() { format!("{} pick", x.class()) } else { x.class().to_owned() };
                        view! {
                            <td class=class data-testid="cell"
                                data-rx=format!("{}/{}", r.device, rc.label) data-tx=tx_attr
                                title=rc.status.clone().unwrap_or_default()
                                on:click=move |_| if let Some(p) = ask.clone() { ctl.pending.set(Some(p)) }>{text}</td>
                        }
                    })
                    .collect_view();
                rows.push(
                    view! {
                        <tr class="rxch">
                            <th class="rxchname">{rc.label.clone()}</th>
                            {cells}
                        </tr>
                    }
                    .into_any(),
                );
            }
            let other = elsewhere(&grid, r);
            if !other.is_empty() {
                let text = other
                    .iter()
                    .map(|(l, s)| format!("{l} ← {s}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                rows.push(view! {
                    <tr class="rxch"><td class="elsewhere" data-testid="elsewhere" colspan=cols.len() + 1>
                        {format!("From devices not listed: {text}")}
                    </td></tr>
                }.into_any());
            }
        }
    }
    view! {
        <p class="status" data-testid="status">{line}</p>
        <div class="matrix-wrap">
            <table class="matrix" data-testid="matrix">
                <thead>
                    <tr><th class="corner" rowspan="2">"RX ↓  TX →"</th>{head_devices}</tr>
                    <tr>{head_channels}</tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
    }
    .into_any()
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
    let class = match (&r.source, r.state.as_deref()) {
        (None, _) => "status none",
        (Some(_), Some("connected")) => "status ok",
        (Some(_), Some("error")) => "status err",
        (Some(_), _) => "status warn",
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

    fn rc(src: Option<(&str, &str)>) -> RxChannel {
        RxChannel {
            channel: 3,
            label: "In 3".into(),
            source: src.map(|(c, d)| Source {
                channel: c.into(),
                device: d.into(),
            }),
            status: None,
            state: None,
        }
    }

    #[test]
    fn a_click_on_an_empty_channel_asks_to_subscribe() {
        let p = pending_for("stage", &rc(None), "desk", "Mix L");
        assert_eq!(p.rx_channel, 3);
        assert_eq!(
            p.target,
            Some(Source {
                channel: "Mix L".into(),
                device: "desk".into()
            })
        );
        assert_eq!(question(&p), "Subscribe In 3 @ stage to Mix L @ desk?");
    }

    #[test]
    fn a_click_on_the_current_source_asks_to_remove_it_case_aside() {
        let p = pending_for("stage", &rc(Some(("mix l", "DESK"))), "desk", "Mix L");
        assert_eq!(p.target, None);
        assert_eq!(
            question(&p),
            "Remove the subscription of In 3 @ stage (now mix l @ DESK)?"
        );
    }

    #[test]
    fn a_click_on_another_source_asks_to_change_it() {
        let p = pending_for("stage", &rc(Some(("Vox", "desk"))), "desk", "Mix L");
        assert_eq!(
            question(&p),
            "Change In 3 @ stage from Vox @ desk to Mix L @ desk?"
        );
    }
}
