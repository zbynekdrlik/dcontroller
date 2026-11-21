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
    /// What shows above the detail: the schema, or one of the lists.
    pub view: RwSignal<MapView>,
    /// The window the colours speak of (#142).
    pub span: RwSignal<map::Span>,
    /// The video network (#124 phase 1), as `/api/video` last gave it.
    pub video: RwSignal<Option<Result<dcontroller_api::VideoReport, String>>>,
    /// The video connection shown (its id) and its reads over the range.
    pub video_sel: RwSignal<Option<String>>,
    pub video_hist: RwSignal<Option<Result<dcontroller_api::VideoHistory, String>>>,
    /// Only the connections in trouble stay bright (#122).
    pub problems: RwSignal<bool>,
    /// A device looked for by name (#122).
    pub find: RwSignal<String>,
}

/// The map's three views of the same network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapView {
    Schema,
    /// Every connection, the worst first.
    Links,
    /// Every device, by name or newest first.
    Devices,
}

impl MapView {
    /// The view [`map::tab_of`] names.
    pub fn of(tab: &str) -> MapView {
        match tab {
            "links" => MapView::Links,
            "devices" => MapView::Devices,
            _ => MapView::Schema,
        }
    }
}

/// Reads the connections' summaries.
pub fn load_links(m: MapCtl) {
    spawn_local(async move {
        let r = pages::get_json::<Links>("/api/links").await;
        // The schema is redrawn only when something changed.
        if m.links.with_untracked(|old| old.as_ref() != Some(&r)) {
            m.links.set(Some(r));
        }
        let v = pages::get_json::<dcontroller_api::VideoReport>("/api/video").await;
        if m.video.with_untracked(|old| old.as_ref() != Some(&v)) {
            m.video.set(Some(v));
        }
    });
}

/// Reads the shown video connection's history over `from`..`to`.
pub fn load_video(m: MapCtl, from: i64, to: i64) {
    let Some(id) = m.video_sel.get_untracked() else {
        return;
    };
    spawn_local(async move {
        let url = format!(
            "/api/video/{}?from={from}&to={to}",
            String::from(js_sys::encode_uri_component(&id))
        );
        let r = pages::get_json::<dcontroller_api::VideoHistory>(&url).await;
        if m.video_sel.get_untracked().as_deref() == Some(id.as_str()) {
            m.video_hist.set(Some(r));
        }
    });
}

/// What kind of connection a chip speaks of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Dante,
    Bridge,
    Video,
}

fn kind_of(sel: &map::Sel) -> Kind {
    match sel {
        map::Sel::Bridge { .. } => Kind::Bridge,
        map::Sel::Video { .. } => Kind::Video,
        _ => Kind::Dante,
    }
}

/// A state's chip; a Dante or video connection's in the window shown, a
/// bridge's in the last hour (its figures cover the hour).
fn chip(state: map::State, kind: Kind) -> (&'static str, String) {
    let span = expect_context::<MapCtl>().span.get_untracked();
    match (state, kind) {
        (map::State::Bad, Kind::Dante) => (
            "chip bad",
            format!("late packets in the last {}", span.words()),
        ),
        (map::State::Bad, Kind::Video) => (
            "chip bad",
            format!("frames lost in the last {}", span.words()),
        ),
        (map::State::Bad, Kind::Bridge) => ("chip bad", "underruns in the last hour".to_owned()),
        (map::State::Warn, Kind::Video) => (
            "chip warn",
            format!("frames duplicated or skipped in the last {}", span.words()),
        ),
        (map::State::Warn, _) => ("chip warn", "peak close to the latency".to_owned()),
        (map::State::Ok, Kind::Dante) => ("chip ok", "no late packets".to_owned()),
        (map::State::Ok, Kind::Video) => ("chip ok", "no frames lost".to_owned()),
        (map::State::Ok, Kind::Bridge) => ("chip ok", "no underruns".to_owned()),
        (map::State::Unknown, _) => ("chip none", "not measured".to_owned()),
    }
}

/// A video connection's words: what it carries and who reports it.
fn video_of(m: MapCtl, id: &str) -> Option<dcontroller_api::VideoConn> {
    m.video.with(|v| {
        v.as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|v| v.connections.iter().find(|c| c.id == id).cloned())
    })
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
        chip: chip(e.state, kind_of(&e.sel)),
        sum,
        bridge,
    }
}

/// One connection of the chosen device, as its table shows it.
struct Row {
    hash: String,
    name: String,
    label: String,
    lost: Option<String>,
    chip: (&'static str, String),
    sum: Option<dcontroller_api::LinkSummary>,
    /// A bridge's own way: no packet history, its underruns as the loss.
    bridge: bool,
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
    // The seconds of the hour with a late packet (#130: the Overview's
    // column, now here).
    let late_s = r.sum.as_ref().map_or_else(dash, |s| map::num(s.late_s_1h));
    let late_s_class = if r.sum.as_ref().is_some_and(|s| s.late_s_1h > 0) {
        "num bad strong"
    } else {
        "num"
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
            view! { <td class=if r.lost.is_some() { "num bad strong" } else { "num" }>{r.lost.clone().unwrap_or_else(|| if r.bridge { "0".to_owned() } else { dash() })}</td> }.into_any(),
            view! { <td class="num">{dash()}</td> }.into_any(),
        ),
    };
    view! {
        <tr class=if on { "click sel" } else { "click" } data-testid="map-dev-row" data-edge=r.hash.clone() on:click=go>
            <td><a class="strong" href=r.hash.clone()>{r.name}</a></td>
            <td>{r.sum.as_ref().map(|s| pages::spark_of(&s.spark, "spark"))}</td>
            <td class="num">{r.label}</td>
            <td class="num">{typical}</td>
            <td class="num">{worst}</td>
            <td class=late_s_class data-testid="late-seconds">{late_s}</td>
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
    let crate::Clk(clk) = expect_context::<crate::Clk>();
    let graph = Memo::new(move |_| match ctl.devices.get() {
        Some(Ok(d)) => {
            let mut g = m
                .links
                .with(|l| map::build(&d, l.as_ref().and_then(|r| r.as_ref().ok()), m.span.get()));
            // Each box's own dsync clock (#124), as the devices say it.
            clk.with(|c| {
                if let Some(Ok(c)) = c {
                    map::with_dsync(&mut g, c);
                }
            });
            // The video network on the same map (#124 phase 1).
            m.video.with(|v| {
                if let Some(Ok(v)) = v {
                    map::with_video(&mut g, v, m.span.get());
                }
            });
            g
        }
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
    // A video connection's history comes from its own read (#124).
    Effect::new(move |_| {
        let id = match shown.get() {
            Some(map::Sel::Video { id }) => Some(id),
            _ => None,
        };
        if m.video_sel.get_untracked() != id {
            m.video_hist.set(None);
            m.video_sel.set(id);
        }
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
    let drawing = move || {
        let show = map::Show {
            problems: m.problems.get(),
            find: m.find.get().trim().to_lowercase(),
        };
        graph.with(|g| map::svg_with(g, &map::layout(g), shown.get().as_ref(), &show))
    };
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
            // A second click on the chosen device lets the whole map back.
            let again =
                matches!(m.chosen.get_untracked(), Some(map::Sel::Device { name: n }) if n == name);
            let hash = if again {
                "#map".to_owned()
            } else {
                map::Sel::Device { name }.hash()
            };
            let _ = window().location().set_hash(&hash);
        } else if matches!(m.chosen.get_untracked(), Some(map::Sel::Device { .. })) {
            // A click beside the boxes and lines does the same.
            let _ = window().location().set_hash("#map");
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
        // A device: everything it receives, else its bridge's both ways.
        Some(map::Sel::Device { .. }) => {
            h.data.with(|d| {
                d.as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .is_some_and(|x| !x.flows.is_empty())
            }) || h
                .bridge
                .with(|b| b.as_ref().is_some_and(|b| !b.points.is_empty()))
        }
        Some(map::Sel::Bridge { .. }) => h
            .bridge
            .with(|b| b.as_ref().is_some_and(|b| !b.points.is_empty())),
        Some(map::Sel::Video { .. }) => m
            .video_hist
            .with(|v| matches!(v, Some(Ok(v)) if !v.points.is_empty())),
        None => false,
    });
    Effect::new(move |_| {
        let sel = shown.get();
        let (data, bridge) = (h.data.get(), h.bridge.get());
        let video = m.video_hist.get();
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
            (Some(map::Sel::Device { .. }), Some(Ok(hd)), _) if !hd.flows.is_empty() => {
                pages::draw_chart(el.unchecked_ref(), &hist::chart_json(&hd, 300));
            }
            (Some(map::Sel::Device { .. }), _, Some(b)) => {
                pages::draw_chart(el.unchecked_ref(), &hist::bridge_chart_json(&b, 300));
            }
            (Some(map::Sel::Video { id }), _, _) => {
                if let Some(Ok(v)) = video {
                    let name =
                        video_of(m, &id).map_or(id, |c| format!("{} → {}", c.source, c.sink));
                    pages::draw_chart(el.unchecked_ref(), &hist::video_chart_json(&v, &name, 300));
                }
            }
            _ => {}
        }
    });
    let title = move || {
        shown.get().map(|s| match s {
            map::Sel::Link { from, to } => format!("{from} → {to}"),
            map::Sel::Device { name } => name,
            map::Sel::Video { id } => {
                video_of(m, &id).map_or(id, |c| format!("{} → {}", c.source, c.sink))
            }
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
        let st = graph.with(|g| map::edge_of(g, &s).map(|e| e.state))?;
        let (class, text) = chip(st, kind_of(&s));
        Some(view! { <span class=class data-testid="map-state">{text}</span> })
    };
    let what =
        move || {
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
            map::Sel::Video { id } => video_of(m, &id).map_or_else(
                || "a video connection its node no longer reports".to_owned(),
                |c| {
                    format!(
                        "video ({}) into {} · reported by {} ({})",
                        c.kind,
                        if c.sink_input.is_empty() { &c.sink } else { &c.sink_input },
                        c.node,
                        if c.direction == "out" { "its output" } else { "its input" }
                    )
                },
            ),
            map::Sel::Device { name } => graph.with(|g| {
                map::node_of(g, &name).map_or_else(String::new, |n| {
                    let node = &g.nodes[n];
                    let clock = node
                        .badge
                        .as_ref()
                        .map_or_else(String::new, |(text, _)| format!(" · {text}"));
                    let dsync = node
                        .dsync
                        .as_ref()
                        .map_or_else(String::new, |(text, _)| format!(" · {text}"));
                    format!("{} · {}{clock}{dsync}", node.lines[0], node.lines[1])
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
            map::Sel::Video { id } => {
                let c = video_of(m, &id);
                let last = c.as_ref().map_or_else(String::new, |c| {
                    format!(
                        "{} in the last hour · {} in 24 h",
                        map::num(c.lost_1h),
                        map::num(c.lost_24h)
                    )
                });
                match m.video_hist.get() {
                    None => view! { <p class="status" data-testid="map-wait">"Reading the video figures…"</p> }.into_any(),
                    Some(Err(e)) => view! { <p class="status error">{format!("Not readable: {e}")}</p> }.into_any(),
                    Some(Ok(v)) => {
                        let sum = |f: fn(&dcontroller_api::VideoPoint) -> u64| v.points.iter().map(f).sum::<u64>();
                        let (lost, dup, skip) = (sum(|p| p.lost), sum(|p| p.duplicated), sum(|p| p.skipped));
                        let most = v.points.iter().filter_map(|p| p.latency_ms).fold(None, |a: Option<f64>, x| Some(a.map_or(x, |a| a.max(x))));
                        let fmt = |x: Option<f64>| x.map_or_else(|| "–".to_owned(), |x| format!("{x:.1} ms"));
                        view! {
                            <div class="figs" data-testid="map-tiles">
                                {tile("map-video-latency", "Latency now".to_owned(), fmt(c.as_ref().and_then(|c| c.latency_ms)), format!("most in this range {}", fmt(most)), false)}
                                {tile("map-video-lost", "Frames lost".to_owned(), map::num(lost), format!("in this range · {last}"), lost > 0)}
                                {tile("map-video-dup", "Frames duplicated".to_owned(), map::num(dup), "in this range: a frame shown twice".to_owned(), dup > 0)}
                                {tile("map-video-skipped", "Frames skipped".to_owned(), map::num(skip), "in this range: dropped as late".to_owned(), skip > 0)}
                            </div>
                        }.into_any()
                    }
                }
            }
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
                // The receiver's other connections, each with its hour (#142,
                // as mockup v2): from one connection to the next without
                // going back to the schema.
                let others: Vec<Row> = graph.with(|g| {
                    let Some(n) = map::node_of(g, &to) else {
                        return Vec::new();
                    };
                    let links = m.links.get();
                    let links = links.as_ref().and_then(|r| r.as_ref().ok());
                    let [ins, outs, inside] = map::edges_of(g, n);
                    ins.into_iter()
                        .map(|e| (e, format!("from {}", g.nodes[e.from].name)))
                        .chain(
                            outs.into_iter()
                                .map(|e| (e, format!("to {}", g.nodes[e.to].name))),
                        )
                        .chain(inside.into_iter().map(|e| (e, String::new())))
                        .filter(|(e, _)| !matches!(&e.sel, map::Sel::Link { from: f, to: t } if f.eq_ignore_ascii_case(&from) && t.eq_ignore_ascii_case(&to)))
                        .map(|(e, name)| row_of(g, e, name, links))
                        .collect()
                });
                let others = (!others.is_empty()).then(|| view! {
                    <h3 class="sub">{format!("Other connections of {to}")}</h3>
                    <div class="table-wrap">
                    <table class="grid links" data-testid="map-others">
                        <thead><tr><th>"Connection"</th><th>"Last hour"</th><th>"Latency"</th><th>"Typical"</th><th>"Worst peak"</th><th>"Late seconds 1 h"</th><th>"Late packets 1 h"</th><th>"Late packets 24 h"</th><th>"State"</th></tr></thead>
                        <tbody>{others.into_iter().map(row).collect_view()}</tbody>
                    </table>
                    </div>
                });
                let changes = format!("#changes/{}", map::encode(&to));
                view! {
                    {table}
                    <h3 class="sub">"Do something"</h3>
                    <p class="actions" data-testid="map-actions">
                        <a class="act" href=dev(&to)>{format!("Change the latency of {to}")}</a>
                        <a class="act" href=dev(&to)>{format!("Device {to}")}</a>
                        <a class="act" href=dev(&from)>{format!("Device {from}")}</a>
                        <a class="act" href="#routing">"Open in Routing"</a>
                        <a class="act" href=changes data-testid="map-changes">{format!("Changes of {to}")}</a>
                    </p>
                    {others}
                }
                .into_any()
            }
            map::Sel::Video { id } => {
                let ends = video_of(m, &id).map(|c| (c.source, c.sink));
                view! {
                    <p class="actions" data-testid="map-actions">
                        {ends.map(|(a, b)| view! {
                            <a class="act" href=map::Sel::Device { name: a.clone() }.hash()>{format!("Device {a}")}</a>
                            <a class="act" href=map::Sel::Device { name: b.clone() }.hash()>{format!("Device {b}")}</a>
                        })}
                    </p>
                }.into_any()
            }
            map::Sel::Bridge { device, .. } => {
                // Its two ways now (#117), as the Devices page showed them.
                let lines = h
                    .bridge
                    .get()
                    .and_then(|b| b.points.last().cloned())
                    .map(pages::bridge_lines);
                let table = h
                    .bridge
                    .get()
                    .as_ref()
                    .and_then(hist::bridge_now)
                    .map(pages::bridge_table);
                view! {
                    {lines}
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
                        <table class="grid links" data-testid=id>
                            <thead><tr><th>"Connection"</th><th>"Last hour"</th><th>"Latency"</th><th>"Typical"</th><th>"Worst peak"</th><th>"Late seconds 1 h"</th><th>"Late packets 1 h"</th><th>"Late packets 24 h"</th><th>"State"</th></tr></thead>
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
        let clocks = expect_context::<Clk>().0.get();
        let [devices, subs, clock, network] =
            pages::device_tiles(&d, clocks.as_ref().and_then(|c| c.as_ref().ok()));
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
        } else if let Some((name, n)) = hl.lost_video.first() {
            let more = match hl.lost_video.len() {
                1 => String::new(),
                k => format!(" and {} more", k - 1),
            };
            (
                "bad",
                format!("losing video: {name} ({} frames lost){more}", map::num(*n)),
            )
        } else if hl.warn > 0 {
            (
                "warn",
                format!(
                    "no late packets in the last {} · {} with a peak close to the latency",
                    m.span.get().words(),
                    hl.warn
                ),
            )
        } else if hl.measured == 0 {
            ("ok", "nothing measured yet".to_owned())
        } else {
            (
                "ok",
                format!("no late packets in the last {}", m.span.get().words()),
            )
        };
        let unmeasured = hl.links - hl.measured;
        let value = format!("{} / {}", hl.clean, hl.measured);
        let sub = if unmeasured > 0 {
            format!("{sub} · {unmeasured} not measured")
        } else {
            sub
        };
        let day = m
            .links
            .with(|l| {
                l.as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .map(map::underruns_24h)
            })
            .unwrap_or(0);
        let mdns = expect_context::<crate::MdnsNow>().0.with(|x| {
            x.as_ref().map(|x| {
                let (value, sub, class) = crate::mdns::tile(x);
                pages::tile(
                    "tile-mdns",
                    "mDNS",
                    value,
                    sub,
                    class,
                    "#network".to_owned(),
                )
            })
        });
        let bridges = (hl.bridges > 0).then(|| {
            pages::tile(
                "tile-bridges",
                "Bridge underruns · 1 h",
                map::num(hl.underruns),
                {
                    let who = match hl.bridges {
                        1 => "1 bridge".to_owned(),
                        n => format!("{n} bridges"),
                    };
                    format!(
                        "{who} · {} in 24 h, blocks played as silence",
                        map::num(day)
                    )
                },
                if hl.underruns > 0 { "bad" } else { "ok" },
                "#map".to_owned(),
            )
        });
        view! {
            <section class="tiles" data-testid="map-tiles-top">
                {pages::tile("tile-connections", "Connections clean", value, sub, class, worst.unwrap_or_else(|| "#map".to_owned()))}
                {bridges}
                {devices}
                {subs}
                {clock}
                {network}
                {mdns}
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
            <table class="grid links" data-testid="map-links">
                <thead><tr><th>"Connection"</th><th>"Last hour"</th><th>"Latency"</th><th>"Typical"</th><th>"Worst peak"</th><th>"Late seconds 1 h"</th><th>"Late packets 1 h"</th><th>"Late packets 24 h"</th><th>"State"</th></tr></thead>
                <tbody>{rows.into_iter().map(|r| { let sel = on.as_deref() == Some(r.hash.as_str()); row_marked(r, sel) }).collect_view()}</tbody>
            </table>
            </div>
            <p class="muted small">"Every connection, the worst on top. A click shows its chart below."</p>
        }
        .into_any()
    };
    // Every device in one table (#122 step 4): the Devices page's list,
    // a row choosing the device like a click on its box.
    let NewestFirst(newest) = expect_context::<NewestFirst>();
    let devices = move || {
        let Some(Ok(d)) = ctl.devices.get() else {
            return ().into_any();
        };
        let start = pages::time_text(d.started_at as i64);
        let on = shown.get().map(|s| s.hash());
        let rows = devlist::order(&d, newest.get())
            .into_iter()
            .map(|dev| {
                let hash = map::Sel::Device { name: dev.name.clone() }.hash();
                let sel = on.as_deref() == Some(hash.as_str());
                let joined = match devlist::joined(&dev, &d) {
                    devlist::Joined::AtStart => format!("there at start ({start})"),
                    devlist::Joined::At(t) => format!("joined {}", pages::time_text(t as i64)),
                };
                // Its connections: how many, and the worst one's state.
                let (count, worst) = graph.with(|g| {
                    map::node_of(g, &dev.name).map_or((0, None), |n| {
                        let all: Vec<&map::Edge> = map::edges_of(g, n).into_iter().flatten().collect();
                        let worst = all.iter().map(|e| (e.state, kind_of(&e.sel))).max();
                        (all.len(), worst)
                    })
                });
                let state = worst.map(|(st, kind)| {
                    let (class, text) = chip(st, kind);
                    view! { <span class=class>{text}</span> }
                });
                let what = [dev.manufacturer.clone(), dev.model.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                let counts = dev.channels.as_ref().map_or_else(|| "not read yet".to_owned(), |c| format!("{} TX · {} RX", c.tx.len(), c.rx.len()));
                let settings = settings_line(&dev.settings.clone().unwrap_or_default(), dev.info.as_ref()).unwrap_or_default();
                let clock = dev.clock.as_ref().map_or_else(String::new, |c| {
                    if c.grandmaster { "grandmaster".to_owned() } else { c.sync.clone() }
                });
                let go = {
                    let hash = hash.clone();
                    move |_| {
                        let _ = window().location().set_hash(&hash);
                    }
                };
                view! {
                    <tr class=if sel { "click sel" } else { "click" } data-testid="map-device-row" data-name=dev.name.clone() on:click=go>
                        <td><a class="strong" href=hash>{dev.name.clone()}</a></td>
                        <td class="muted">{what}</td>
                        <td>{counts}</td>
                        <td>{settings}</td>
                        <td>{clock}</td>
                        <td class="mono">{dev.address.clone()}</td>
                        <td class="muted">{joined}</td>
                        <td>{if count == 0 { "none".to_owned() } else { format!("{count} · ") }}{state}</td>
                    </tr>
                }
            })
            .collect_view();
        let seg = move |on: bool, label: &'static str, id: &'static str| {
            view! {
                <button data-testid=id class=move || if newest.get() == on { "seg on" } else { "seg" }
                    on:click=move |_| newest.set(on)>{label}</button>
            }
        };
        view! {
            <div class="bar">
                <div class="seg-group" data-testid="map-device-sort">
                    {seg(false, "By name", "map-sort-name")}
                    {seg(true, "Newest first", "map-sort-newest")}
                </div>
            </div>
            <div class="table-wrap">
            <table class="grid" data-testid="map-devices">
                <thead><tr><th>"Device"</th><th>"Model"</th><th>"Channels"</th><th>"Settings"</th><th>"Clock"</th><th>"Address"</th><th>"On the network"</th><th>"Connections"</th></tr></thead>
                <tbody>{rows}</tbody>
            </table>
            </div>
            <p class="muted small">"Every device. A click shows its detail below."</p>
        }
        .into_any()
    };
    let tab = move |view: MapView, label: &'static str, id: &'static str| {
        view! {
            <button data-testid=id class=move || if m.view.get() == view { "seg on" } else { "seg" }
                on:click=move |_| m.view.set(view)>{label}</button>
        }
    };
    view! {
        {tiles_top}
        <div class="bar">
            <div class="seg-group" data-testid="map-view">
                {tab(MapView::Schema, "Schema", "map-view-schema")}
                {tab(MapView::Links, "Links", "map-view-links")}
                {tab(MapView::Devices, "Devices", "map-view-devices")}
            </div>
            // What the colours speak of: the last hour, 3 hours or day.
            <div class="seg-group" data-testid="map-span" title="The window the colours of the connections speak of">
                {map::Span::ALL.map(|s| view! {
                    <button data-testid=format!("map-span-{}", s.label().replace(' ', ""))
                        class=move || if m.span.get() == s { "seg on" } else { "seg" }
                        on:click=move |_| m.span.set(s)>{s.label()}</button>
                }).collect_view()}
            </div>
            <span class="bar-group" style=move || if m.view.get() == MapView::Schema { "" } else { "display:none" }>
                <input class="find" type="search" data-testid="map-find" placeholder="find a device"
                    aria-label="Find a device on the map"
                    prop:value=move || m.find.get()
                    on:input=move |ev| m.find.set(event_target_value(&ev))/>
                <button data-testid="map-problems" title="Dim the connections with no problem"
                    aria-pressed=move || m.problems.get().to_string()
                    class=move || if m.problems.get() { "seg solo on" } else { "seg solo" }
                    on:click=move |_| m.problems.update(|p| *p = !*p)>"Problems only"</button>
            </span>
            <span class="status" data-testid="status">{status}</span>
        </div>
        <section class="panel" data-testid="map-links-panel"
            style=move || if m.view.get() == MapView::Links { "" } else { "display:none" }>
            {move || (m.view.get() == MapView::Links).then(table)}
        </section>
        <section class="panel" data-testid="map-devices-panel"
            style=move || if m.view.get() == MapView::Devices { "" } else { "display:none" }>
            {move || (m.view.get() == MapView::Devices).then(devices)}
        </section>
        <section class="panel" data-testid="map-panel"
            style=move || if m.view.get() == MapView::Schema { "" } else { "display:none" }>
            <div class="map-wrap" data-testid="map" on:click=click
                on:mouseover=move |ev| focus(ev, true) on:mouseout=move |ev| focus(ev, false)
                inner_html=drawing></div>
            <div class="legend">
                <span><i class="ok"></i>"no late packets"</span>
                <span><i class="warn"></i>"peak close to the latency"</span>
                <span><i class="bad"></i>{move || format!("late packets (underruns for a bridge) in the last {}: audio lost", m.span.get().words())}</span>
                <span>"colours speak of the window on the right of the tabs: 1 h, 3 h or 24 h"</span>
                <span><i class="unknown"></i>"not measured"</span>
                <span>"number on a line: the receiver's latency for that connection"</span>
                <span>"click a device: only its connections stay bright; click it again for all"</span>
            </div>
        </section>
        <section class="panel" data-testid="map-detail"
            style=move || if shown.get().is_some() { "" } else { "display:none" }>
            <header class="panel-head">
                <b data-testid="map-title">{title}</b>
                {state}
                {chosen_by}
                <span class="grow"></span>
                <span class="panel-head" style=move || if has_chart.get() || !matches!(shown.get(), Some(map::Sel::Device { .. })) { "margin:0" } else { "display:none" }>
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
