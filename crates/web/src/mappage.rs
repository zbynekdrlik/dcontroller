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
    /// The Links table shows in place of the schema.
    pub table: RwSignal<bool>,
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

/// A connection's row: `name` for a Dante connection, the bridge's own
/// wording for its two ways.
fn row_of(g: &map::Graph, e: &map::Edge, name: String, links: Option<&Links>) -> Row {
    let sum = match &e.sel {
        map::Sel::Link { from, to } => map::summary_of(links, from, to).cloned(),
        _ => None,
    };
    let bridge = matches!(e.sel, map::Sel::Bridge { .. });
    Row {
        hash: e.sel.hash(),
        name: match e.sel {
            map::Sel::Bridge { to_dante: true, .. } => format!("{} → Dante", g.nodes[e.from].name),
            map::Sel::Bridge { .. } => format!("Dante → {}", g.nodes[e.to].name),
            _ => name,
        },
        label: e.label.clone(),
        lost: e.lost.clone(),
        chip: chip(e.state, bridge),
        sum,
    }
}

/// One connection of the chosen device, as its table shows it.
struct Row {
    hash: String,
    name: String,
    label: String,
    lost: Option<String>,
    chip: (&'static str, &'static str),
    sum: Option<dcontroller_api::LinkSummary>,
}

fn row(r: Row) -> impl IntoView {
    row_marked(r, false)
}

/// A connection's row; `on` marks the one that shows below.
fn row_marked(r: Row, on: bool) -> impl IntoView {
    let go = {
        let hash = r.hash.clone();
        move |_| {
            let _ = window().location().set_hash(&hash);
        }
    };
    let dash = || "–".to_owned();
    let late = |n: u64| {
        let class = if n == 0 { "num" } else { "num bad strong" };
        view! { <td class=class>{map::num(n)}</td> }
    };
    let (typical, worst, late_1h, late_24h) = match &r.sum {
        Some(s) => (
            hist::ms(s.typical_us),
            hist::ms(s.peak_us),
            late(s.late_1h).into_any(),
            late(s.late_24h).into_any(),
        ),
        None => (
            dash(),
            dash(),
            view! { <td class=if r.lost.is_some() { "num bad strong" } else { "num" }>{r.lost.clone().unwrap_or_else(dash)}</td> }.into_any(),
            view! { <td class="num">{dash()}</td> }.into_any(),
        ),
    };
    view! {
        <tr class=if on { "click sel" } else { "click" } data-testid="map-dev-row" data-edge=r.hash.clone() on:click=go>
            <td><a class="strong" href=r.hash.clone()>{r.name}</a></td>
            <td>{pages::spark_of(r.sum.as_ref().map_or(&[], |s| s.spark.as_slice()), "spark")}</td>
            <td class="num">{r.label}</td>
            <td class="num">{typical}</td>
            <td class="num">{worst}</td>
            {late_1h}
            {late_24h}
            <td><span class=r.chip.0>{r.chip.1}</span></td>
        </tr>
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
                .filter(|c| map::has(g, c))
                .or_else(|| map::worst(g).map(|e| e.sel.clone()))
        })
    });
    // Its history: the receiver's (a bridge's own), read by the app's
    // history poll.
    Effect::new(move |_| {
        let device = shown.get().and_then(|s| s.device().map(str::to_owned));
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
                .set_hash(&map::Sel::Device { name }.hash());
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
        Some(map::Sel::Device { .. }) => false,
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
            map::Sel::Device { name } => name,
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
            map::Sel::Device { name } => graph.with(|g| {
                map::node_of(g, &name).map_or_else(String::new, |n| {
                    let node = &g.nodes[n];
                    let clock = node
                        .badge
                        .as_ref()
                        .map_or_else(String::new, |(text, _)| format!(" · {text}"));
                    format!("{} · {}{clock}", node.lines[0], node.lines[1])
                })
            }),
        })
    };
    let chosen_by = move || {
        let auto = shown.get().is_some()
            && !graph.with(|g| m.chosen.get().is_some_and(|c| map::has(g, &c)));
        auto.then(|| view! { <span class="muted small" data-testid="map-auto">"the worst connection now · click a line for another"</span> })
    };
    // The figures over the chosen range, then the last hour and day.
    let tiles = move || {
        let Some(sel) = shown.get() else {
            return ().into_any();
        };
        match sel {
            map::Sel::Device { .. } => ().into_any(),
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
                let dev = |name: &str| {
                    map::Sel::Device {
                        name: name.to_owned(),
                    }
                    .hash()
                };
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
                        <a class="act" href=map::Sel::Device { name: device.clone() }.hash()>{format!("Device {device}")}</a>
                    </p>
                }.into_any()
            }
            map::Sel::Device { name } => {
                let dev = ctl.devices.with(|d| match d {
                    Some(Ok(d)) => d
                        .devices
                        .iter()
                        .find(|x| hist::same_name(&x.name, &name))
                        .cloned(),
                    _ => None,
                });
                let Some(dev) = dev else {
                    return ().into_any();
                };
                // Its connections: into it, out of it, inside it.
                let groups: Vec<(&'static str, &'static str, Vec<Row>)> = graph.with(|g| {
                    let Some(n) = map::node_of(g, &name) else {
                        return Vec::new();
                    };
                    let [ins, outs, inside] = map::edges_of(g, n);
                    let links = m.links.get();
                    let links = links.as_ref().and_then(|r| r.as_ref().ok());
                    let rows = |edges: Vec<&map::Edge>,
                                other: fn(&map::Edge) -> usize,
                                word: &str|
                     -> Vec<Row> {
                        edges
                            .into_iter()
                            .map(|e| {
                                let name = format!("{word} {}", g.nodes[other(e)].name);
                                row_of(g, e, name, links)
                            })
                            .collect()
                    };
                    vec![
                        ("Receives", "map-dev-in", rows(ins, |e| e.from, "from")),
                        ("Sends", "map-dev-out", rows(outs, |e| e.to, "to")),
                        (
                            "Inside (its ASIO bridge)",
                            "map-dev-bridge",
                            rows(inside, |e| e.from, ""),
                        ),
                    ]
                });
                let none = groups.iter().all(|(_, _, r)| r.is_empty());
                let tables = groups
                    .into_iter()
                    .filter(|(_, _, r)| !r.is_empty())
                    .map(|(title, id, rows)| view! {
                        <h3 class="sub">{title}</h3>
                        <div class="table-wrap">
                        <table class="grid" data-testid=id>
                            <thead><tr><th>"Connection"</th><th>"Last hour"</th><th>"Latency"</th><th>"Typical"</th><th>"Worst peak"</th><th>"Lost 1 h"</th><th>"Lost 24 h"</th><th>"State"</th></tr></thead>
                            <tbody>{rows.into_iter().map(row).collect_view()}</tbody>
                        </table>
                        </div>
                    })
                    .collect_view();
                view! {
                    {tables}
                    {none.then(|| view! { <p class="muted" data-testid="map-dev-none">"No connection: nothing is subscribed to this device and it is subscribed to nothing."</p> })}
                    <h3 class="sub">"Device"</h3>
                    <div class="device map-device" data-testid="map-device-body"><DeviceBody dev=dev/></div>
                }.into_any()
            }
        }
    };
    // The network's last hour at a glance (#122 step 3): the tiles the
    // overview has, with the connections in place of the receivers.
    let tiles_top = move || {
        let Some(Ok(d)) = ctl.devices.get() else {
            return ().into_any();
        };
        let [devices, subs, clock, network] = pages::device_tiles(&d);
        let (hl, worst) = graph.with(|g| (map::health(g), map::worst(g).map(|e| e.sel.hash())));
        let (class, sub) = if let Some((name, n)) = hl.lost.first() {
            let more = match hl.lost.len() {
                1 => String::new(),
                k => format!(" and {} more", k - 1),
            };
            (
                "bad",
                format!("losing audio: {name} ({} late){more}", map::num(*n)),
            )
        } else if hl.warn > 0 {
            (
                "warn",
                format!(
                    "no late packets in the last hour · {} close to the latency or late earlier today",
                    hl.warn
                ),
            )
        } else if hl.measured == 0 {
            ("ok", "nothing measured yet".to_owned())
        } else {
            ("ok", "no late packets in the last hour".to_owned())
        };
        let unmeasured = hl.links - hl.measured;
        let value = format!("{} / {}", hl.clean, hl.measured);
        let sub = if unmeasured > 0 {
            format!("{sub} · {unmeasured} not measured")
        } else {
            sub
        };
        let bridges = (hl.bridges > 0).then(|| {
            pages::tile(
                "tile-bridges",
                "Bridge underruns · 1 h",
                map::num(hl.underruns),
                match hl.bridges {
                    1 => "1 bridge, blocks played as silence".to_owned(),
                    n => format!("{n} bridges, blocks played as silence"),
                },
                if hl.underruns > 0 { "bad" } else { "ok" },
                "#map".to_owned(),
            )
        });
        view! {
            <section class="tiles" data-testid="map-tiles-top">
                {pages::tile("tile-connections", "Connections clean · 1 h", value, sub, class, worst.unwrap_or_else(|| "#map".to_owned()))}
                {bridges}
                {devices}
                {subs}
                {clock}
                {network}
            </section>
        }
        .into_any()
    };
    // Every connection, the worst first.
    let table = move || {
        let links = m.links.get();
        let links = links.as_ref().and_then(|r| r.as_ref().ok());
        let rows: Vec<Row> = graph.with(|g| {
            map::worst_first(g)
                .into_iter()
                .map(|e| {
                    let name = format!("{} → {}", g.nodes[e.from].name, g.nodes[e.to].name);
                    row_of(g, e, name, links)
                })
                .collect()
        });
        let on = shown.get().map(|s| s.hash());
        if rows.is_empty() {
            return view! { <p class="muted" data-testid="map-links-none">"No connection: nothing is subscribed yet."</p> }.into_any();
        }
        view! {
            <div class="table-wrap">
            <table class="grid" data-testid="map-links">
                <thead><tr><th>"Connection"</th><th>"Last hour"</th><th>"Latency"</th><th>"Typical"</th><th>"Worst peak"</th><th>"Lost 1 h"</th><th>"Lost 24 h"</th><th>"State"</th></tr></thead>
                <tbody>{rows.into_iter().map(|r| { let sel = on.as_deref() == Some(r.hash.as_str()); row_marked(r, sel) }).collect_view()}</tbody>
            </table>
            </div>
            <p class="muted small">"Every connection, the worst on top. A click shows its chart below."</p>
        }
        .into_any()
    };
    let tab = move |table: bool, label: &'static str, id: &'static str| {
        view! {
            <button data-testid=id class=move || if m.table.get() == table { "seg on" } else { "seg" }
                on:click=move |_| m.table.set(table)>{label}</button>
        }
    };
    view! {
        {tiles_top}
        <div class="bar">
            <div class="seg-group" data-testid="map-view">
                {tab(false, "Schema", "map-view-schema")}
                {tab(true, "Links", "map-view-links")}
            </div>
            <span class="status" data-testid="status">{status}</span>
        </div>
        <section class="panel" data-testid="map-links-panel"
            style=move || if m.table.get() { "" } else { "display:none" }>
            {move || m.table.get().then(table)}
        </section>
        <section class="panel" data-testid="map-panel"
            style=move || if m.table.get() { "display:none" } else { "" }>
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
                <span class="panel-head" style=move || if matches!(shown.get(), Some(map::Sel::Device { .. })) { "display:none" } else { "margin:0" }>
                    <RangeHead prefix="map"/>
                </span>
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
