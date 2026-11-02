//! The overview and the latency history pages. Both stay mounted while
//! the device list refreshes (they read it inside), so the chart keeps its
//! zoom and is updated in place, never rebuilt by a poll.

use super::*;
use dcontroller_api::{FlowHistory, History, Overview, ReceiverSummary};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// `charts.js`: draws or updates the latency chart in `el`.
    #[wasm_bindgen(js_namespace = dcChart, js_name = latency)]
    fn chart_latency(el: &JsValue, json: &str);
}

/// How often the latency page reads the history again while it shows a
/// live range (one that ends now and spans at most a day).
pub const HIST_POLL: Duration = Duration::from_secs(5);
/// How often the overview summary is read again.
pub const OVERVIEW_POLL: Duration = Duration::from_secs(10);

/// The latency page's state: the receiver (kept in the hash
/// `#latency/<name>`), the range, a zoom into it, the last history read,
/// and the overview summary both pages share.
#[derive(Clone, Copy)]
pub struct Hist {
    pub device: RwSignal<Option<String>>,
    pub range: RwSignal<i64>,
    pub zoom: RwSignal<Option<(i64, i64)>>,
    pub data: RwSignal<Option<Result<History, String>>>,
    pub overview: RwSignal<Option<Result<Overview, String>>>,
    pub scale: RwSignal<hist::Scale>,
    pub reads: StoredValue<hist::HistReads>,
}

impl Hist {
    /// Whether the shown range ends now (and is short enough to follow).
    pub fn live(&self) -> bool {
        self.zoom.get_untracked().is_none() && self.range.get_untracked() <= 24 * 3600
    }
}

fn now_s() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    let res = gloo_net::http::Request::get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("server answered {}", res.status()));
    }
    res.json::<T>().await.map_err(|e| e.to_string())
}

/// Reads the chosen receiver's history for the chosen range or zoom; a
/// live poll (`poll`) waits while a read runs.
pub fn load_history(h: Hist, poll: bool) {
    if poll && h.reads.get_value().busy() {
        return;
    }
    let Some(dev) = h.device.get_untracked() else {
        return;
    };
    let (from, to) = h
        .zoom
        .get_untracked()
        .unwrap_or_else(|| (now_s() - h.range.get_untracked(), now_s()));
    let url = format!(
        "/api/history/{}?from={from}&to={to}",
        String::from(js_sys::encode_uri_component(&dev))
    );
    let mut reads = h.reads.get_value();
    let n = reads.start();
    h.reads.set_value(reads);
    spawn_local(async move {
        let r = get_json::<History>(&url).await;
        // Only the newest read's answer shows (a range, zoom or device
        // chosen since makes this one stale).
        let mut reads = h.reads.get_value();
        let show = reads.done(n);
        h.reads.set_value(reads);
        if show {
            h.data.set(Some(r));
        }
    });
}

/// Reads the overview summary.
pub fn load_overview(h: Hist) {
    spawn_local(async move {
        h.overview
            .set(Some(get_json::<Overview>("/api/overview").await))
    });
}

fn summary_of<'a>(
    o: &'a Option<Result<Overview, String>>,
    name: &str,
) -> Option<&'a ReceiverSummary> {
    o.as_ref()?
        .as_ref()
        .ok()?
        .receivers
        .iter()
        .find(|r| r.device.eq_ignore_ascii_case(name))
}

/// A small sparkline of a receiver's last hour (peak as a share of the latency).
fn spark(r: Option<&ReceiverSummary>, class: &'static str) -> AnyView {
    let path = r
        .map(|r| hist::spark_path(&r.spark, 120.0, 28.0, 1000))
        .unwrap_or_default();
    // The latency line sits at 1000 per mille unless a peak passed it.
    let top = r
        .and_then(|r| r.spark.iter().flatten().copied().max())
        .unwrap_or(0)
        .max(1000);
    let limit = 28.0 - 28.0 * 1000.0 / f64::from(top);
    view! {
        <svg class=class viewBox="0 0 120 28" preserveAspectRatio="none" aria-hidden="true">
            <line class="limit" x1="0" x2="120" y1=format!("{limit:.1}") y2=format!("{limit:.1}") />
            <path class="line" d=path />
        </svg>
    }
    .into_any()
}

/// One health tile of the overview.
fn tile(
    id: &'static str,
    title: &'static str,
    value: String,
    sub: String,
    class: &'static str,
    href: &'static str,
) -> impl IntoView {
    view! {
        <a class=format!("tile {class}") href=href data-testid=id>
            <span class="tile-title">{title}</span>
            <span class="tile-value">{value}</span>
            <span class="tile-sub">{sub}</span>
        </a>
    }
}

/// The overview: the network's health at a glance, and every receiver's
/// last hour.
#[component]
pub fn OverviewPage() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let h = expect_context::<Hist>();
    let tiles = move || {
        let Some(Ok(d)) = ctl.devices.get() else {
            return view! { <p class="status" data-testid="status">"Loading…"</p> }.into_any();
        };
        let read = d.devices.iter().filter(|x| x.channels.is_some()).count();
        let failing = d.devices.iter().filter(|x| x.error.is_some()).count();
        let subs: Vec<&RxChannel> = d
            .devices
            .iter()
            .filter_map(|x| x.channels.as_ref())
            .flat_map(|c| c.rx.iter())
            .filter(|r| r.source.is_some())
            .collect();
        let not_rx = subs.iter().filter(|r| !r.receiving()).count();
        let crow = clock::rows(&d);
        let (cline, cwarn) = clock::summary(&crow);
        let leader = clock::leaders(&crow).first().map(|s| (*s).to_owned());
        let nrow = net::rows(&d);
        let (_, nwarn) = net::summary(&nrow);
        let slow = nrow.iter().filter(|r| r.warn).count();
        let nread = nrow.iter().filter(|r| r.primary.is_some()).count();
        let o = h.overview.get();
        let recs = o
            .as_ref()
            .and_then(|o| o.as_ref().ok())
            .map(|o| o.receivers.clone())
            .unwrap_or_default();
        let late: u64 = recs.iter().map(|r| r.late_1h).sum();
        let worst = recs
            .iter()
            .filter(|r| r.latency_us > 0)
            .max_by_key(|r| u64::from(r.peak_us) * 1000 / u64::from(r.latency_us));
        let lat_sub = match worst {
            Some(w) => format!(
                "closest: {} at {} of {}",
                w.device,
                hist::ms(w.peak_us),
                hist::ms(w.latency_us)
            ),
            None => format!("{} receivers recorded", recs.len()),
        };
        view! {
            <section class="tiles">
                {tile("tile-devices", "Devices", format!("{}", d.devices.len()),
                    if failing > 0 { format!("{read} read · {failing} not answering") } else { format!("{read} read, all answering") },
                    if failing > 0 { "warn" } else { "ok" }, "#devices")}
                {tile("tile-latency", "Late packets · 1 h", format!("{late}"), lat_sub,
                    if late > 0 { "bad" } else { "ok" }, "#latency")}
                {tile("tile-subs", "Subscriptions", format!("{}", subs.len()),
                    if not_rx > 0 { format!("{not_rx} not receiving") } else { "all receiving".to_owned() },
                    if not_rx > 0 { "warn" } else { "ok" }, "#routing")}
                {tile("tile-clock", "Clock", leader.unwrap_or_else(|| "no leader".to_owned()), cline,
                    if cwarn { "warn" } else { "ok" }, "#clock")}
                {tile("tile-network", "Network", format!("{nread} links"),
                    if slow > 0 { format!("{slow} slow or down") } else { "every link at full speed".to_owned() },
                    if nwarn { "warn" } else { "ok" }, "#network")}
            </section>
        }
        .into_any()
    };
    let receivers = move || {
        let o = h.overview.get();
        let list = match &o {
            None => {
                return view! { <p class="status">"Reading the latency history…"</p> }.into_any();
            }
            Some(Err(e)) => {
                return view! { <p class="status error">{format!("History not readable: {e}")}</p> }
                    .into_any();
            }
            Some(Ok(o)) => o.receivers.clone(),
        };
        if list.is_empty() {
            return view! { <p class="status" data-testid="overview-empty">"No receiver recorded in the last hour yet."</p> }.into_any();
        }
        view! {
            <div class="table-wrap">
            <table class="grid" data-testid="overview-receivers">
                <thead><tr><th>"Receiver"</th><th>"Last hour"</th><th>"Peak"</th><th>"Latency"</th><th>"Late 1 h"</th><th>"Late 24 h"</th><th>"State"</th></tr></thead>
                <tbody>
                    {list.into_iter().map(|r| {
                        let (class, text) = hist::health(&r);
                        let href = format!("#latency/{}", String::from(js_sys::encode_uri_component(&r.device)));
                        let name = r.device.clone();
                        let pick = {
                            let name = name.clone();
                            move |_| {
                                h.zoom.set(None);
                                h.data.set(None);
                                h.device.set(Some(name.clone()));
                            }
                        };
                        view! {
                            <tr data-testid="overview-receiver" data-name=name.clone()>
                                <td><a href=href on:click=pick class="strong">{name.clone()}</a></td>
                                <td>{spark(Some(&r), "spark")}</td>
                                <td class="num">{hist::ms(r.peak_us)}</td>
                                <td class="num">{if r.latency_us > 0 { hist::ms(r.latency_us) } else { "–".to_owned() }}</td>
                                <td class="num">{r.late_1h}</td>
                                <td class="num">{r.late_24h}</td>
                                <td><span class=format!("chip {class}")>{text}</span></td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
            </div>
        }
        .into_any()
    };
    view! {
        {tiles}
        <section class="panel">
            <header class="panel-head"><h2>"Receivers"</h2><span class="muted">"packet latency, recorded all the time"</span></header>
            {receivers}
        </section>
    }
}

/// The latency history of one receiver: a chart per range with zoom, and
/// each flow's figures and distribution over it.
#[component]
pub fn LatencyPage() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let h = expect_context::<Hist>();
    let chart_ref = NodeRef::<leptos::html::Div>::new();
    let wired = StoredValue::new(false);
    // Draw on every new read; wire the zoom events once.
    Effect::new(move |_| {
        let data = h.data.get();
        let scale = h.scale.get();
        let Some(el) = chart_ref.get() else {
            return;
        };
        if !wired.get_value() {
            wired.set_value(true);
            let on_zoom =
                Closure::<dyn Fn(web_sys::CustomEvent)>::new(move |ev: web_sys::CustomEvent| {
                    let d = ev.detail();
                    let get = |k: &str| {
                        js_sys::Reflect::get(&d, &JsValue::from_str(k))
                            .ok()
                            .and_then(|v| v.as_f64())
                    };
                    if let (Some(from), Some(to)) = (get("from"), get("to")) {
                        h.zoom.set(Some((from as i64, to as i64)));
                    }
                });
            let on_unzoom = Closure::<dyn Fn()>::new(move || h.zoom.set(None));
            let target: &web_sys::EventTarget = el.unchecked_ref();
            let _ = target
                .add_event_listener_with_callback("dc-zoom", on_zoom.as_ref().unchecked_ref());
            let _ = target
                .add_event_listener_with_callback("dc-unzoom", on_unzoom.as_ref().unchecked_ref());
            on_zoom.forget();
            on_unzoom.forget();
        }
        if let Some(Ok(hd)) = data.filter(|d| d.as_ref().is_ok_and(|x| !x.flows.is_empty())) {
            chart_latency(el.unchecked_ref(), &hist::chart_json(&hd, 320, scale));
        }
    });
    let receivers = move || {
        let names: Vec<String> = match ctl.devices.get() {
            Some(Ok(d)) => d
                .devices
                .iter()
                .filter(|x| x.channels.as_ref().is_some_and(|c| !c.rx.is_empty()))
                .map(|x| x.name.clone())
                .collect(),
            _ => Vec::new(),
        };
        let o = h.overview.get();
        names
            .into_iter()
            .map(|name| {
                let s = summary_of(&o, &name).cloned();
                let (class, text) = s
                    .as_ref()
                    .map_or(("none", "not recorded yet".to_owned()), hist::health);
                let href = format!(
                    "#latency/{}",
                    String::from(js_sys::encode_uri_component(&name))
                );
                let on = {
                    let n = name.clone();
                    move || h.device.get().as_deref() == Some(n.as_str())
                };
                let pick = {
                    let n = name.clone();
                    move |_| {
                        h.zoom.set(None);
                        h.data.set(None);
                        h.device.set(Some(n.clone()));
                    }
                };
                view! {
                    <a href=href data-testid="lat-receiver" data-name=name.clone()
                        class=move || if on() { "recv on" } else { "recv" } on:click=pick>
                        <span class="recv-name">{name.clone()}</span>
                        {spark(s.as_ref(), "spark small")}
                        <span class=format!("chip {class}")>{text}</span>
                    </a>
                }
            })
            .collect_view()
    };
    let ranges = move || {
        hist::RANGES
            .iter()
            .map(|&(label, secs)| {
                view! {
                    <button data-testid="lat-range" data-range=secs.to_string()
                        class=move || if h.zoom.get().is_none() && h.range.get() == secs { "seg on" } else { "seg" }
                        on:click=move |_| { h.zoom.set(None); h.range.set(secs); }>{label}</button>
                }
            })
            .collect_view()
    };
    let head = move || {
        let zoom = h.zoom.get().map(|(a, b)| {
            let t = |s: i64| {
                String::from(
                    js_sys::Date::new(&JsValue::from_f64(s as f64 * 1000.0))
                        .to_locale_string("sk-SK", &JsValue::UNDEFINED),
                )
            };
            view! {
                <button class="chip info" data-testid="lat-unzoom" on:click=move |_| h.zoom.set(None)>
                    {format!("zoom {} – {} ✕", t(a), t(b))}
                </button>
            }
        });
        let live = (h.zoom.get().is_none() && h.range.get() <= 24 * 3600)
            .then(|| view! { <span class="live" data-testid="lat-live">"live"</span> });
        let scale_btn = |s: hist::Scale, label: &'static str, id: &'static str| {
            view! {
                <button data-testid=id class=move || if h.scale.get() == s { "seg on" } else { "seg" }
                    on:click=move |_| h.scale.set(s)>{label}</button>
            }
        };
        view! {
            <div class="seg-group">{ranges}</div>
            <div class="seg-group">
                {scale_btn(hist::Scale::Share, "% of latency", "lat-scale-share")}
                {scale_btn(hist::Scale::Ms, "ms", "lat-scale-ms")}
            </div>
            {zoom}
            {live}
        }
    };
    let figures = move || {
        let Some(name) = h.device.get() else {
            return view! { <p class="status">"Pick a receiver."</p> }.into_any();
        };
        match h.data.get() {
            None => view! { <p class="status" data-testid="lat-wait">"Reading the history…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="status error">{format!("Not readable: {e}")}</p> }.into_any(),
            Some(Ok(hd)) if hd.flows.is_empty() => view! {
                <p class="status" data-testid="lat-noflows">{format!("Nothing recorded for {name} in this range yet: it receives no flow, or the recorder only started.")}</p>
            }
            .into_any(),
            Some(Ok(hd)) => {
                let step = if hd.step_s < 60 { format!("{} s", hd.step_s) } else if hd.step_s < 3600 { format!("{} min", hd.step_s / 60) } else { format!("{} h", hd.step_s / 3600) };
                let tier = hd.tier_s;
                view! {
                    <p class="muted small" data-testid="lat-head">{format!("{name} · {} flows · one point per {step} (largest peak per point)", hd.flows.len())}</p>
                    <div class="table-wrap">
                    <table class="grid" data-testid="lat-flows">
                        <thead><tr><th></th><th>"Flow"</th><th>"From"</th><th>"Channels"</th><th>"Latency"</th><th>"Peak"</th><th>"Mean"</th><th>"Late"</th><th>"Past latency"</th><th>"Distribution"</th></tr></thead>
                        <tbody>
                            {hd.flows.iter().enumerate().map(|(i, f)| flow_row(i, f, tier)).collect_view()}
                        </tbody>
                    </table>
                    </div>
                }
                .into_any()
            }
        }
    };
    view! {
        <div class="split">
            <aside class="receivers" data-testid="lat-receivers">{receivers}</aside>
            <section class="panel grow">
                <header class="panel-head">{head}</header>
                <div class="chart" node_ref=chart_ref data-testid="lat-chart"
                    style=move || if h.data.get().is_some_and(|d| d.is_ok_and(|x| !x.flows.is_empty())) { "" } else { "display:none" }></div>
                <p class="muted small hint">"Drag across the chart to zoom in; double-click to zoom out."</p>
                {figures}
            </section>
        </div>
    }
}

fn flow_row(i: usize, f: &FlowHistory, tier_s: i64) -> impl IntoView {
    let top = f.histogram.iter().copied().max().unwrap_or(0).max(1);
    let edges = dcontroller_api::latency_bins(f.latency_us);
    let n_ok = edges
        .iter()
        .take_while(|&&e| e <= f.latency_us.max(1))
        .count();
    let chans = if f.channels.len() > 3 {
        format!(
            "{}, {} … ({} ch)",
            f.channels[0],
            f.channels[1],
            f.channels.len()
        )
    } else {
        f.channels.join(", ")
    };
    view! {
        <tr data-testid="lat-flow">
            <td><span class="dot" style=format!("background:{}", hist::color(i))></span></td>
            <td class="strong">{format!("Flow {}", f.slot)}</td>
            <td>{if f.sources.is_empty() { "?".to_owned() } else { f.sources.join(", ") }}</td>
            <td class="muted" title=f.channels.join(", ")>{chans}</td>
            <td class="num" data-testid="lat-flow-latency">{if f.latency_us > 0 { hist::ms(f.latency_us) } else { "–".to_owned() }}</td>
            <td class="num" data-testid="lat-flow-peak">{hist::ms(f.peak_us)}</td>
            <td class="num">{hist::ms(f.avg_us)}</td>
            <td class={if f.late_total > 0 { "num bad" } else { "num" }} data-testid="lat-flow-late">{f.late_total}</td>
            <td class={if f.over > 0 { "num warn" } else { "num" }}>{hist::over_text(f.over, tier_s)}</td>
            <td>
                <span class="hist" title="distribution of the peaks: eighths of the latency, then 1.5×, 2× and over">
                    {f.histogram.iter().enumerate().map(|(k, &c)| {
                        let pct = if c == 0 { 0.0 } else { (c as f64 / top as f64 * 100.0).max(6.0) };
                        let class = if k >= n_ok { "hb late" } else { "hb" };
                        view! { <span class=class style=format!("height:{pct:.0}%") title=c.to_string()></span> }
                    }).collect_view()}
                </span>
            </td>
        </tr>
    }
}
