//! The Map page (dcontroller #122): the schema of what is connected to
//! what, and under it the chart of the connection the user clicked (the
//! worst one until then). The schema is redrawn on any change; the panel
//! under it stays mounted, so its chart keeps its zoom.

use super::*;
use dcontroller_api::Links;
use pages::{Hist, RangeHead};
use wasm_bindgen::JsCast;

/// The map's state: the connections' last hour, and the connection the
/// hash names (`None`: the worst one shows).
#[derive(Clone, Copy)]
pub struct MapCtl {
    pub links: RwSignal<Option<Result<Links, String>>>,
    pub chosen: RwSignal<Option<map::Sel>>,
}

/// Reads the connections' summaries.
pub fn load_links(m: MapCtl) {
    spawn_local(async move {
        let r = pages::get_json::<Links>("/api/links").await;
        // The schema is redrawn only when something changed.
        if m.links.with_untracked(|old| old.as_ref() != Some(&r)) {
            m.links.set(Some(r));
        }
    });
}

fn chip(state: map::State, bridge: bool) -> (&'static str, &'static str) {
    match (state, bridge) {
        (map::State::Bad, false) => ("chip bad", "late packets in the last hour"),
        (map::State::Bad, true) => ("chip bad", "underruns in the last hour"),
        (map::State::Warn, _) => ("chip warn", "close to the latency, or late earlier today"),
        (map::State::Ok, false) => ("chip ok", "no late packets"),
        (map::State::Ok, true) => ("chip ok", "no underruns"),
        (map::State::Unknown, _) => ("chip none", "not measured"),
    }
}

/// One figure of the chosen connection.
fn tile(id: &'static str, key: String, value: String, sub: String, bad: bool) -> impl IntoView {
    view! {
        <div class=if bad { "fig bad" } else { "fig" } data-testid=id>
            <div class="k">{key}</div>
            <div class="v">{value}</div>
            <div class="s">{sub}</div>
        </div>
    }
}

#[component]
pub fn MapPage() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let h = expect_context::<Hist>();
    let m = expect_context::<MapCtl>();
    let graph = Memo::new(move |_| match ctl.devices.get() {
        Some(Ok(d)) => m
            .links
            .with(|l| map::build(&d, l.as_ref().and_then(|r| r.as_ref().ok()))),
        _ => map::Graph::default(),
    });
    // The connection that shows: the chosen one while the network has it,
    // else the worst.
    let shown = Memo::new(move |_| {
        graph.with(|g| {
            m.chosen
                .get()
                .and_then(|c| map::edge_of(g, &c))
                .or_else(|| map::worst(g))
                .map(|e| e.sel.clone())
        })
    });
    // Its history: the receiver's (a bridge's own), read by the app's
    // history poll.
    Effect::new(move |_| {
        let device = shown.get().map(|s| s.device().to_owned());
        let same = h.device.with_untracked(|d| match (d, &device) {
            (Some(a), Some(b)) => hist::same_name(a, b),
            (None, None) => true,
            _ => false,
        });
        if !same {
            h.zoom.set(None);
            h.data.set(None);
            h.bridge.set(None);
            h.device.set(device);
        }
    });
    let drawing = move || graph.with(|g| map::svg(g, &map::layout(g), shown.get().as_ref()));
    let click = move |ev: leptos::ev::MouseEvent| {
        let Some(target) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        if let Ok(Some(edge)) = target.closest("[data-edge]") {
            if let Some(hash) = edge.get_attribute("data-edge") {
                let _ = window().location().set_hash(&hash);
            }
        } else if let Ok(Some(node)) = target.closest("g.node:not(.asio):not(.missing)")
            && let Some(name) = node.get_attribute("data-node")
        {
            let _ = window()
                .location()
                .set_hash(&format!("#devices/{}", map::encode(&name)));
        }
    };
    // Pointing at a device brings its connections forward, with their
    // numbers (a dense schema keeps the good ones quiet).
    let focus = move |ev: leptos::ev::MouseEvent, on: bool| {
        let Some(target) = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        else {
            return;
        };
        let (Ok(Some(node)), Ok(Some(svg))) = (target.closest("g.node"), target.closest("svg.map"))
        else {
            return;
        };
        let Some(name) = node.get_attribute("data-node") else {
            return;
        };
        let _ = svg.class_list().toggle_with_force("focus-on", on);
        if let Ok(edges) = svg.query_selector_all("[data-edge]") {
            for i in 0..edges.length() {
                let Some(e) = edges
                    .item(i)
                    .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
                else {
                    continue;
                };
                let ends = [e.get_attribute("data-from"), e.get_attribute("data-to")];
                let mine = ends.iter().flatten().any(|x| *x == name);
                let _ = e.class_list().toggle_with_force("focus", on && mine);
            }
        }
    };
    let status = move || match ctl.devices.get() {
        None => "Loading…".to_owned(),
        Some(Err(e)) => format!("Server not reachable: {e}"),
        Some(Ok(d)) => {
            let line = status_line(&d);
            let lines = graph.with(|g| g.edges.len());
            match lines {
                0 => format!("{line} · nothing is subscribed yet"),
                1 => format!("{line} · 1 connection"),
                n => format!("{line} · {n} connections"),
            }
        }
    };
    let chart_ref = NodeRef::<leptos::html::Div>::new();
    let wired = StoredValue::new(false);
    // The chosen connection's chart, on every new read and every choice.
    let has_chart = Memo::new(move |_| match shown.get() {
        Some(map::Sel::Link { from, .. }) => h.data.with(|d| {
            d.as_ref()
                .and_then(|r| r.as_ref().ok())
                .is_some_and(|x| !hist::only_source(x, &from).flows.is_empty())
        }),
        Some(map::Sel::Bridge { .. }) => h
            .bridge
            .with(|b| b.as_ref().is_some_and(|b| !b.points.is_empty())),
        None => false,
    });
    Effect::new(move |_| {
        let sel = shown.get();
        let (data, bridge) = (h.data.get(), h.bridge.get());
        let Some(el) = chart_ref.get() else {
            return;
        };
        if !wired.get_value() {
            wired.set_value(true);
            pages::wire_zoom(el.unchecked_ref(), h);
        }
        if !has_chart.get() {
            return;
        }
        match (sel, data, bridge) {
            (Some(map::Sel::Link { from, .. }), Some(Ok(hd)), _) => {
                pages::draw_chart(el.unchecked_ref(), &hist::link_chart_json(&hd, &from, 300));
            }
            (Some(map::Sel::Bridge { to_dante, .. }), _, Some(b)) => {
                pages::draw_chart(
                    el.unchecked_ref(),
                    &hist::bridge_dir_chart_json(&b, to_dante, 300),
                );
            }
            _ => {}
        }
    });
    let title = move || {
        shown.get().map(|s| match s {
            map::Sel::Link { from, to } => format!("{from} → {to}"),
            map::Sel::Bridge { device, to_dante } => {
                let name = graph.with(|g| {
                    map::edge_of(
                        g,
                        &map::Sel::Bridge {
                            device: device.clone(),
                            to_dante,
                        },
                    )
                    .map(|e| g.nodes[if to_dante { e.from } else { e.to }].name.clone())
                    .unwrap_or_else(|| "ASIO".to_owned())
                });
                if to_dante {
                    format!("{name} → Dante")
                } else {
                    format!("Dante → {name}")
                }
            }
        })
    };
    let state = move || {
        let s = shown.get()?;
        let bridge = matches!(s, map::Sel::Bridge { .. });
        let st = graph.with(|g| map::edge_of(g, &s).map(|e| e.state))?;
        let (class, text) = chip(st, bridge);
        Some(view! { <span class=class data-testid="map-state">{text}</span> })
    };
    let what = move || {
        shown.get().map(|s| match s {
            map::Sel::Link { from, to } => {
                let pairs: Vec<String> = ctl.devices.with(|d| match d {
                    Some(Ok(d)) => d
                        .devices
                        .iter()
                        .find(|x| hist::same_name(&x.name, &to))
                        .and_then(|x| x.channels.as_ref())
                        .map(|c| {
                            c.rx.iter()
                                .filter_map(|r| {
                                    let s = r.source.as_ref()?;
                                    hist::same_name(&s.device, &from)
                                        .then(|| format!("{} → {}", s.channel, r.label))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                });
                let n = pairs.len();
                let list = if n > 6 {
                    format!("{} … {}", pairs[..3].join(", "), pairs[n - 1])
                } else {
                    pairs.join(", ")
                };
                format!("{n} channel{}: {list}", if n == 1 { "" } else { "s" })
            }
            map::Sel::Bridge { device, .. } => {
                format!(
                    "how long the bridge {device} holds a sample between its ASIO side and Dante"
                )
            }
        })
    };
    let chosen_by = move || {
        let auto = shown.get().is_some()
            && !graph.with(|g| {
                m.chosen
                    .get()
                    .is_some_and(|c| map::edge_of(g, &c).is_some())
            });
        auto.then(|| view! { <span class="muted small" data-testid="map-auto">"the worst connection now · click a line for another"</span> })
    };
    // The figures over the chosen range, then the last hour and day.
    let tiles = move || {
        let Some(sel) = shown.get() else {
            return ().into_any();
        };
        match sel {
            map::Sel::Link { from, to } => {
                let sum = m.links.with(|l| {
                    map::summary_of(l.as_ref().and_then(|r| r.as_ref().ok()), &from, &to).cloned()
                });
                let last = sum.as_ref().map_or_else(
                    || "last hour: not measured".to_owned(),
                    |s| {
                        format!(
                            "{} in the last hour · {} in 24 h",
                            map::num(s.late_1h),
                            map::num(s.late_24h)
                        )
                    },
                );
                match h.data.get() {
                    None => view! { <p class="status" data-testid="map-wait">"Reading the history…"</p> }.into_any(),
                    Some(Err(e)) => view! { <p class="status error">{format!("Not readable: {e}")}</p> }.into_any(),
                    Some(Ok(hd)) => {
                        let one = hist::only_source(&hd, &from);
                        let Some(s) = hist::by_source(&one).into_iter().next() else {
                            return view! {
                                <p class="status" data-testid="map-nothing">{format!("Nothing recorded for {from} → {to} in this range: {to} reports no packet times for it, or the recorder only started.")}</p>
                            }.into_any();
                        };
                        let lat = s.latencies.iter().map(|&l| hist::ms(l)).collect::<Vec<_>>().join(", ");
                        let over = s.peak_latency_us > 0 && s.peak_us > s.peak_latency_us;
                        let share = if s.peak_latency_us > 0 {
                            format!("{} % of the latency", u64::from(s.peak_us) * 100 / u64::from(s.peak_latency_us))
                        } else {
                            String::new()
                        };
                        view! {
                            <div class="figs" data-testid="map-tiles">
                                {tile("map-latency", format!("Latency (set on {to})"), if lat.is_empty() { "–".to_owned() } else { lat }, "a packet later than this is not played".to_owned(), false)}
                                {tile("map-typical", "Typical lateness".to_owned(), hist::typical_us(&s).map_or_else(|| "–".to_owned(), hist::ms), "most packets arrive this late".to_owned(), false)}
                                {tile("map-worst", "Worst peak".to_owned(), hist::ms(s.peak_us), if over { "over the latency".to_owned() } else { share }, over)}
                                {tile("map-late", "Late packets (not played)".to_owned(), map::num(s.late_total), format!("in this range · {last}"), s.late_total > 0)}
                            </div>
                        }.into_any()
                    }
                }
            }
            map::Sel::Bridge { to_dante, .. } => {
                match h.bridge.get().as_ref().and_then(|b| hist::bridge_dir(b, to_dante)) {
                    None => view! { <p class="status" data-testid="map-wait">"Reading the bridge's figures…"</p> }.into_any(),
                    Some(d) => {
                        let cause = match d.cause.as_str() {
                            "none" | "" => String::new(),
                            "driver" => " · latest cause: driver callback late".to_owned(),
                            other => format!(" · latest cause: {other}"),
                        };
                        view! {
                            <div class="figs" data-testid="map-tiles">
                                {tile("map-held", "Held in the bridge".to_owned(), hist::ms(d.now_us), if d.measured { "now, from one card to the other".to_owned() } else { "now, from its settings (the device gives no measured time)".to_owned() }, false)}
                                {tile("map-most", "Most in this range".to_owned(), hist::ms(d.most_us), String::new(), false)}
                                {tile("map-underruns", "Underruns in this range".to_owned(), map::num(d.underruns), format!("blocks played as silence{cause}"), d.underruns > 0)}
                            </div>
                        }.into_any()
                    }
                }
            }
        }
    };
    // Under the chart: the connection's flows and distribution (a bridge:
    // its parts), and where to go from here.
    let below = move || {
        let Some(sel) = shown.get() else {
            return ().into_any();
        };
        match sel {
            map::Sel::Link { from, to } => {
                let rows = h.data.with(|d| {
                    d.as_ref().and_then(|r| r.as_ref().ok()).map(|hd| {
                        let one = hist::only_source(hd, &from);
                        (hist::by_source(&one), hd.tier_s)
                    })
                });
                let table = rows.filter(|(g, _)| !g.is_empty()).map(|(groups, tier)| {
                    let body: Vec<AnyView> = groups
                        .iter()
                        .map(|s| pages::source_row(hist::LINK_COLOR.to_owned(), s, tier).into_any())
                        .collect();
                    view! {
                        <div class="table-wrap">
                        <table class="grid" data-testid="map-flows">
                            <thead><tr><th></th><th>"From"</th><th>"Late (not played)"</th><th>"Worst peak"</th><th>"Latency"</th><th>"Past latency"</th><th>"Distribution"</th></tr></thead>
                            <tbody>{body}</tbody>
                        </table>
                        </div>
                    }
                });
                let dev = |name: &str| format!("#devices/{}", map::encode(name));
                view! {
                    {table}
                    <p class="actions" data-testid="map-actions">
                        <a class="act" href=dev(&to)>{format!("Device {to}: settings, latency")}</a>
                        <a class="act" href=dev(&from)>{format!("Device {from}")}</a>
                        <a class="act" href="#routing">"Routing"</a>
                    </p>
                }
                .into_any()
            }
            map::Sel::Bridge { device, .. } => {
                let table = h
                    .bridge
                    .get()
                    .as_ref()
                    .and_then(hist::bridge_now)
                    .map(pages::bridge_table);
                view! {
                    {table}
                    <p class="actions" data-testid="map-actions">
                        <a class="act" href=format!("#devices/{}", map::encode(&device))>{format!("Device {device}")}</a>
                    </p>
                }.into_any()
            }
        }
    };
    view! {
        <p class="status" data-testid="status">{status}</p>
        <section class="panel" data-testid="map-panel">
            <div class="map-wrap" data-testid="map" on:click=click
                on:mouseover=move |ev| focus(ev, true) on:mouseout=move |ev| focus(ev, false)
                inner_html=drawing></div>
            <div class="legend">
                <span><i class="ok"></i>"no late packets"</span>
                <span><i class="warn"></i>"close to the latency, or late earlier today"</span>
                <span><i class="bad"></i>"late packets or underruns in the last hour (audio lost)"</span>
                <span><i class="unknown"></i>"not measured"</span>
                <span>"number on a line: the receiver's latency for that connection"</span>
                <span>"point at a device: its connections stand out"</span>
            </div>
        </section>
        <section class="panel" data-testid="map-detail"
            style=move || if shown.get().is_some() { "" } else { "display:none" }>
            <header class="panel-head">
                <b data-testid="map-title">{title}</b>
                {state}
                {chosen_by}
                <span class="grow"></span>
                <RangeHead prefix="map"/>
            </header>
            <p class="muted small" data-testid="map-what">{what}</p>
            {tiles}
            <div class="chart" node_ref=chart_ref data-testid="map-chart"
                style=move || if has_chart.get() { "" } else { "display:none" }></div>
            <p class="muted small hint" style=move || if has_chart.get() { "" } else { "display:none" }>
                "The line: how late packets arrive (a bridge: how long it holds a sample). Red bars at the bottom: audio lost at that time. Drag across the chart to zoom in; double-click to zoom out."
            </p>
            {below}
        </section>
    }
}
