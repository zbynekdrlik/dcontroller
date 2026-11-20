//! dcontroller dashboard: the Dante devices the server discovers, with
//! their channels and subscriptions, and the routing matrix. A click on a
//! cross-point asks for a subscription change; nothing is sent before the
//! user confirms it, and the server reads it back and logs it. The version label is
//! build-injected from the same `buildver` as the server's `/api/version`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use dcontroller_api::{
    ChangeEntry, ChangeResult, ChannelLevel, Device, Devices, Info, Mdns, MeterReport,
    MulticastCreate, MulticastDelete, Rename, RenameTarget, RxChannel, SettingChange, Settings,
    Source, SubscriptionChange, TxFlows,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

mod changes;
mod clock;
mod devlist;
mod flows;
mod hist;
mod lat;
mod map;
mod mappage;
mod matrix;
mod mdns;
mod mtr;
mod net;
mod pages;
mod parts;
mod settings;
use matrix::{Grid, affected, cell, elsewhere, problems, tally};

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
    provide_context(NewestFirst(RwSignal::new(false)));
    let changes_only = RwSignal::new(hash_device("#changes/"));
    provide_context(ChangesOnly(changes_only));
    provide_context(Expanded {
        tx: RwSignal::new(BTreeSet::new()),
        rx: RwSignal::new(BTreeSet::new()),
        scroll: StoredValue::new((0, 0)),
        problems: StoredValue::new(false),
        hover: RwSignal::new(String::new()),
    });
    let view_now = RwSignal::new(View::from_hash());
    let ctl = Ctl {
        devices,
        pending: RwSignal::new(None),
        renaming: RwSignal::new(None),
        setting: RwSignal::new(None),
        multicast: RwSignal::new(None),
        mc_delete: RwSignal::new(None),
        flows: RwSignal::new(BTreeMap::new()),
        busy: RwSignal::new(false),
        result: RwSignal::new(None),
    };
    provide_context(ctl);
    let hst = pages::Hist {
        device: RwSignal::new(None),
        range: RwSignal::new(hist::DEFAULT_RANGE),
        zoom: RwSignal::new(None),
        data: RwSignal::new(None),
        bridge: RwSignal::new(None),
        reads: StoredValue::new(hist::HistReads::default()),
    };
    provide_context(hst);
    let mp = mappage::MapCtl {
        links: RwSignal::new(None),
        chosen: RwSignal::new(map::Sel::from_hash(
            &window().location().hash().unwrap_or_default(),
        )),
        // A phone holds one column of the schema: it opens on the table of
        // connections, the schema one tap away (#123).
        view: RwSignal::new(
            if let Some(t) = map::tab_of(&window().location().hash().unwrap_or_default()) {
                mappage::MapView::of(t)
            } else if window()
                .inner_width()
                .ok()
                .and_then(|w| w.as_f64())
                .is_some_and(|w| w < 700.0)
            {
                mappage::MapView::Links
            } else {
                mappage::MapView::Schema
            },
        ),
        span: RwSignal::new(map::Span::default()),
    };
    provide_context(mp);
    // The routing matrix takes each connection's state from the same read.
    provide_context(LinkStates(Memo::new(move |_| {
        mp.links
            .with(|l| matrix::link_states(l.as_ref().and_then(|r| r.as_ref().ok()), mp.span.get()))
    })));
    // The hash is the page's address: a link, back or forward moves the view
    // (and the chosen latency receiver) with it.
    {
        let on_hash = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
            record_use();
            let v = View::from_hash();
            if view_now.get_untracked() != v {
                view_now.set(v);
            }
            if v == View::Map {
                let hash = window().location().hash().unwrap_or_default();
                if let Some(t) = map::tab_of(&hash) {
                    mp.view.set(mappage::MapView::of(t));
                }
                let c = map::Sel::from_hash(&hash);
                if mp.chosen.get_untracked() != c {
                    let picked = c.is_some();
                    mp.chosen.set(c);
                    // A choice brings its detail and chart into view (#142):
                    // under a tall schema they were a screen below.
                    if picked {
                        set_timeout(
                            || {
                                if let Ok(Some(e)) =
                                    document().query_selector(r#"[data-testid="map-detail"]"#)
                                {
                                    e.scroll_into_view();
                                }
                            },
                            Duration::from_millis(60),
                        );
                    }
                }
            }
            if v == View::Changes {
                let d = hash_device("#changes/");
                if changes_only.get_untracked() != d {
                    changes_only.set(d);
                }
            }
        });
        let _ = window().add_event_listener_with_callback(
            "hashchange",
            wasm_bindgen::JsCast::unchecked_ref(on_hash.as_ref()),
        );
        on_hash.forget();
        // The page as it opened counts too.
        record_use();
    }
    // The history of the chosen receiver: at once on any change, then
    // every few seconds while the range is live.
    Effect::new(move |_| {
        let (_, _, _, v) = (
            hst.device.get(),
            hst.range.get(),
            hst.zoom.get(),
            view_now.get(),
        );
        if v == View::Map && hst.device.get().is_some() {
            pages::load_history(hst, false);
        }
    });
    let _ = set_interval_with_handle(
        move || {
            if view_now.get_untracked() == View::Map
                && hst.device.get_untracked().is_some()
                && hst.live()
            {
                pages::load_history(hst, true);
            }
        },
        pages::HIST_POLL,
    );
    // The map's connections: at once on the page, then with the overview.
    Effect::new(move |_| {
        if matches!(view_now.get(), View::Map | View::Routing) {
            mappage::load_links(mp);
        }
    });
    let _ = set_interval_with_handle(
        move || {
            if matches!(view_now.get_untracked(), View::Map | View::Routing) {
                mappage::load_links(mp);
            }
        },
        pages::OVERVIEW_POLL,
    );
    let mtr = Mtr {
        device: RwSignal::new(hash_device("#meters/")),
        report: RwSignal::new(None),
    };
    provide_context(mtr);
    // The devices' dsync clock figures, while the clock view is open (#124).
    let clk = Clk(RwSignal::new(None));
    provide_context(clk);
    let load_clocks = move || {
        // The clock page, and the pages whose clock tile counts dsync too.
        if matches!(view_now.get_untracked(), View::Clock | View::Map) {
            spawn_local(async move {
                clk.0.set(Some(
                    pages::get_json::<dcontroller_api::Clocks>("/api/clocks").await,
                ));
            });
        }
    };
    Effect::new(move |_| {
        view_now.track();
        load_clocks();
    });
    let _ = set_interval_with_handle(load_clocks, CLOCKS_POLL);
    // The chosen device's levels, often while the meters view is open.
    let poll_meters = move || {
        if view_now.get_untracked() != View::Meters {
            return;
        }
        let Some(dev) = mtr.device.get_untracked() else {
            return;
        };
        spawn_local(async move {
            let r = fetch_meters(&dev).await;
            if mtr.device.get_untracked().as_deref() == Some(dev.as_str()) {
                mtr.report.set(Some(r));
            }
        });
    };
    let _ = set_interval_with_handle(poll_meters, MTR_POLL);
    // The matrix's devices' levels, while the routing view is open.
    let lv = Levels(RwSignal::new(HashMap::new()));
    provide_context(lv);
    let lv_busy = RwSignal::new(false);
    let poll_levels = move || {
        // The matrix's headers, and the meters page while it shows every
        // device at once (no device chosen).
        let all_meters =
            view_now.get_untracked() == View::Meters && mtr.device.get_untracked().is_none();
        if (view_now.get_untracked() != View::Routing && !all_meters) || lv_busy.get_untracked() {
            return;
        }
        let names: Vec<String> = devices.with_untracked(|d| match d {
            Some(Ok(d)) => d
                .devices
                .iter()
                .filter(|dev| {
                    dev.channels
                        .as_ref()
                        .is_some_and(|c| !c.tx.is_empty() || !c.rx.is_empty())
                })
                .map(|dev| dev.name.clone())
                .collect(),
            _ => Vec::new(),
        });
        if names.is_empty() {
            return;
        }
        lv_busy.set(true);
        spawn_local(async move {
            match fetch_levels(&names).await {
                Ok(r) => lv.0.set(
                    r.into_iter()
                        .map(|m| (m.device.to_ascii_lowercase(), m))
                        .collect(),
                ),
                Err(e) => leptos::logging::log!("levels: {e}"),
            }
            lv_busy.set(false);
        });
    };
    let _ = set_interval_with_handle(poll_levels, MTR_POLL);
    Effect::new(move |_| {
        let _ = (mtr.device.get(), view_now.get());
        poll_meters();
    });
    // An answer equal to the one shown changes nothing: setting it would
    // rebuild the open view (the matrix's scroll, a click half done).
    let load = move || {
        spawn_local(async move {
            let new = Some(fetch_devices().await);
            if devices.with_untracked(|d| *d != new) {
                devices.set(new);
            }
        })
    };
    load();
    // The interval lives as long as the page.
    let _ = set_interval_with_handle(load, POLL);
    // The mDNS meter, for the alarm on every page.
    let md = MdnsNow(RwSignal::new(None));
    provide_context(md);
    let load_mdns = move || {
        spawn_local(async move {
            match fetch_mdns().await {
                Ok(m) => {
                    let new = Some(m);
                    if md.0.with_untracked(|o| *o != new) {
                        md.0.set(new);
                    }
                }
                Err(e) => leptos::logging::log!("mdns: {e}"),
            }
        })
    };
    load_mdns();
    let _ = set_interval_with_handle(load_mdns, POLL);
    // A tab open across a deploy keeps running the old app: when the
    // server answers another version, load the new one, once no dialog
    // or change is open.
    let watch_version = move || {
        spawn_local(async move {
            let Ok(server) = fetch_version().await else {
                return;
            };
            let quiet = ctl.pending.get_untracked().is_none()
                && ctl.renaming.get_untracked().is_none()
                && ctl.setting.get_untracked().is_none()
                && !ctl.busy.get_untracked();
            if new_version(VERSION, &server) && quiet {
                leptos::logging::log!("server runs {server}, this page {VERSION}: reloading");
                let _ = window().location().reload();
            }
        })
    };
    let _ = set_interval_with_handle(watch_version, VERSION_POLL);
    // Which page shows, and whether the device list is there: the
    // overview and latency pages stay mounted across device refreshes.
    let page = Memo::new(move |_| {
        let state = devices.with(|d| match d {
            None => 0,
            Some(Err(_)) => 1,
            Some(Ok(_)) => 2,
        });
        (view_now.get(), state)
    });
    let iface = move || match devices.get() {
        Some(Ok(d)) if d.enabled => format!("Dante on {}", d.interface.unwrap_or_default()),
        Some(Ok(_)) => "Dante discovery off".to_owned(),
        Some(Err(_)) => "server not reachable".to_owned(),
        None => "connecting…".to_owned(),
    };
    view! {
        <div class="app">
        <aside class="side">
            <div class="brand">{logo()}<span>"dcontroller"</span></div>
            <nav class="nav">
                {View::ALL.into_iter().map(|v| view! {
                    <a href=v.hash() data-testid=format!("tab-{}", v.slug())
                        class=move || if view_now.get() == v { "nav-item on" } else { "nav-item" }
                        on:click=move |_| view_now.set(v)>
                        {icon(v)}<span>{v.title()}</span>
                    </a>
                }).collect_view()}
            </nav>
            <div class="side-foot">
                <span class="iface" data-testid="iface">{iface}</span>
                <span class="version" data-testid="version">{VERSION}</span>
            </div>
        </aside>
        <main class="main">
            <header class="page-head"><h1>{move || view_now.get().title()}</h1></header>
            {move || md.0.with(|m| m.as_ref().and_then(mdns::alarm)).map(|line| view! {
                <p class="alarm" role="alert" data-testid="mdns-alarm">{line}</p>
            })}
            {move || md.0.with(|m| m.as_ref().and_then(mdns::browser_alarm)).map(|line| view! {
                <p class="alarm" role="alert" data-testid="browser-alarm">{line}</p>
            })}
            {move || md.0.with(|m| m.as_ref().and_then(mdns::floor_alarm)).map(|line| view! {
                <p class="alarm" role="alert" data-testid="floor-alarm">{line}</p>
            })}
            {move || match page.get() {
                (_, 0) => view! { <p class="status" data-testid="status">"Loading…"</p> }.into_any(),
                (_, 1) => view! {
                    <p class="status error" data-testid="status">{move || match devices.get() {
                        Some(Err(e)) => format!("Server not reachable: {e}"),
                        _ => String::new(),
                    }}</p>
                }.into_any(),
                (View::Map, _) => view! { <mappage::MapPage/> }.into_any(),
                (v, _) => (move || match devices.get() {
                    Some(Ok(d)) => match v {
                        View::Routing => view! { <Routing d=d/> }.into_any(),
                        View::Changes => view! { <ChangeLog/> }.into_any(),
                        View::Meters => view! { <MeterView d=d/> }.into_any(),
                        View::Clock => view! { <ClockView d=d/> }.into_any(),
                        View::Network => view! { <NetworkView d=d/> }.into_any(),
                        View::Map => ().into_any(),
                    },
                    _ => ().into_any(),
                }).into_any(),
            }}
        </main>
        </div>
        <ConfirmDialog/>
        <RenameDialog/>
        <SettingDialog/>
        <MulticastDialog/>
        <MulticastDeleteDialog/>
        <ResultLine/>
    }
}

/// The page's view, kept in the URL hash.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Map,
    Routing,
    Meters,
    Clock,
    Network,
    Changes,
}

impl View {
    /// The views in the sidebar's order.
    /// The Overview and Devices pages live on the Map now (#130).
    const ALL: [View; 6] = [
        View::Map,
        View::Routing,
        View::Meters,
        View::Clock,
        View::Network,
        View::Changes,
    ];

    fn slug(self) -> &'static str {
        match self {
            View::Map => "map",
            View::Routing => "routing",
            View::Meters => "meters",
            View::Clock => "clock",
            View::Network => "network",
            View::Changes => "changes",
        }
    }

    fn title(self) -> &'static str {
        match self {
            View::Map => "Map",
            View::Routing => "Routing",
            View::Meters => "Meters",
            View::Clock => "Clock",
            View::Network => "Network",
            View::Changes => "Changes",
        }
    }

    fn hash(self) -> String {
        format!("#{}", self.slug())
    }

    fn from_hash() -> View {
        let hash = window().location().hash().unwrap_or_default();
        let head = hash
            .trim_start_matches('#')
            .split('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        // An old page's link lands on the Map (#130), latency's too (#106).
        if let Some(new) = map::moved(&hash) {
            let _ = window().location().replace(&new);
            return View::Map;
        }
        View::ALL
            .into_iter()
            .find(|v| v.slug() == head)
            .unwrap_or(View::Map)
    }
}

/// The sidebar's icon for a view (inline SVG, stroke icons).
fn icon(v: View) -> impl IntoView {
    let d = match v {
        View::Map => {
            "M3 4h6v5H3zM15 4h6v5h-6zM9 15h6v5H9zM9 6.5h6M6 9v3.5a2 2 0 0 0 2 2h1M18 9v3.5a2 2 0 0 1-2 2h-1"
        }
        View::Routing => {
            "M4 4h4v4H4zM16 4h4v4h-4zM4 16h4v4H4zM16 16h4v4h-4zM8 6h8M6 8v8M18 8v8M8 18h8"
        }
        View::Meters => "M5 20V10M10 20V4M15 20v-7M20 20V8",
        View::Clock => "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 7v5l3 3",
        View::Network => "M12 3v6M5 21v-4h14v4M12 9v8M5 13h14M5 13v4M19 13v4",
        View::Changes => "M4 6h16M4 12h10M4 18h7M17 15l3 3-3 3",
    };
    view! {
        <svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8"
            stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d=d /></svg>
    }
}

/// The brand mark: a waveform in a rounded square.
fn logo() -> impl IntoView {
    view! {
        <svg class="logo" viewBox="0 0 32 32" aria-hidden="true">
            <rect x="1" y="1" width="30" height="30" rx="8" fill="var(--accent)" />
            <path d="M7 16h3l2-6 3 12 3-9 2 3h5" fill="none" stroke="#0b0f17" stroke-width="2.4"
                stroke-linecap="round" stroke-linejoin="round" />
        </svg>
    }
}

/// The device named in the hash after `prefix` (`#meters/<name>`).
fn hash_device(prefix: &str) -> Option<String> {
    let hash = window().location().hash().unwrap_or_default();
    let name = hash.strip_prefix(prefix)?;
    js_sys::decode_uri_component(name)
        .ok()
        .map(String::from)
        .filter(|n| !n.is_empty())
}

/// How often the meters view asks for the chosen device's levels.
const MTR_POLL: Duration = Duration::from_millis(300);
/// How often the clock view reads the devices' dsync figures.
const CLOCKS_POLL: Duration = Duration::from_secs(4);

/// The devices' dsync clock figures, as last read.
#[derive(Clone, Copy)]
struct Clk(RwSignal<Option<Result<dcontroller_api::Clocks, String>>>);

/// The device the Changes page is narrowed to (`#changes/<device>`).
#[derive(Clone, Copy)]
struct ChangesOnly(RwSignal<Option<String>>);

/// Every connection's state, for the routing matrix.
#[derive(Clone, Copy)]
struct LinkStates(Memo<matrix::LinkStates>);

/// The meters view's device (kept in the hash `#meters/<name>`) and its last report.
#[derive(Clone, Copy)]
struct Mtr {
    device: RwSignal<Option<String>>,
    report: RwSignal<Option<Result<MeterReport, String>>>,
}

async fn fetch_meters(device: &str) -> Result<MeterReport, String> {
    let url = format!(
        "/api/meters/{}",
        String::from(js_sys::encode_uri_component(device))
    );
    let res = gloo_net::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("server answered {}", res.status()));
    }
    res.json::<MeterReport>().await.map_err(|e| e.to_string())
}

/// How often an open page asks the server for its version.
const VERSION_POLL: Duration = Duration::from_secs(15);

async fn fetch_mdns() -> Result<Mdns, String> {
    let res = gloo_net::http::Request::get("/api/mdns")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("server answered {}", res.status()));
    }
    res.json().await.map_err(|e| e.to_string())
}

/// The last mDNS meter read, for the alarm and the Network page.
#[derive(Clone, Copy)]
struct MdnsNow(RwSignal<Option<Mdns>>);

async fn fetch_version() -> Result<String, String> {
    let res = gloo_net::http::Request::get("/api/version")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("server answered {}", res.status()));
    }
    res.text().await.map_err(|e| e.to_string())
}

/// The server answers a version other than the page's own (an empty or
/// non-version answer, such as a login page, is no new version).
fn new_version(page: &str, server: &str) -> bool {
    let server = server.trim();
    server.starts_with('v') && server.len() < 64 && server != page
}

/// The routing matrix's live levels, by device name (lower case).
#[derive(Clone, Copy)]
struct Levels(RwSignal<HashMap<String, MeterReport>>);

async fn fetch_levels(names: &[String]) -> Result<Vec<MeterReport>, String> {
    let url = format!(
        "/api/levels?devices={}",
        String::from(js_sys::encode_uri_component(&names.join(",")))
    );
    let res = gloo_net::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("server answered {}", res.status()));
    }
    res.json::<Vec<MeterReport>>()
        .await
        .map_err(|e| e.to_string())
}

/// "no meters" next to a device in the matrix when it sends no meter
/// frames: its bars show no level of its own, which is not silence.
#[component]
fn NoMeters(device: String) -> impl IntoView {
    let Levels(lv) = expect_context::<Levels>();
    let key = device.to_ascii_lowercase();
    move || {
        lv.with(|m| mtr::no_meters(m.get(&key))).then(|| view! {
            <span class="chip nometer" data-testid="no-meters"
                title="This device sends no meters: its bars show no level of its own (a receiver's show its source's, hatched). Silence is not shown here.">"no meters"</span>
        })
    }
}

/// A live level bar in a matrix header: `device`'s TX (`tx`) or RX
/// channel `channel`, or for a folded device its loudest. Vertical in a
/// TX channel's column, horizontal elsewhere.
fn level_bar(
    device: &str,
    tx: bool,
    channel: Option<u16>,
    vertical: bool,
    sources: Vec<(String, u16)>,
) -> impl IntoView {
    let Levels(lv) = expect_context::<Levels>();
    let key = device.to_ascii_lowercase();
    // An RX bar of a receiver that sends no meters shows its sources' level.
    let from_source = {
        let key = key.clone();
        let sources = sources.clone();
        move || !tx && !sources.is_empty() && lv.with(|m| mtr::own_rx(m.get(&key)).is_none())
    };
    let now = move || {
        lv.with(|m| {
            if tx {
                return m.get(&key).and_then(|r| mtr::pick(&r.tx, channel).cloned());
            }
            match mtr::own_rx(m.get(&key)) {
                Some(rx) => mtr::pick(rx, channel).cloned(),
                None => mtr::source_level(m, &sources),
            }
        })
    };
    let src2 = from_source.clone();
    let now2 = now.clone();
    let now3 = now.clone();
    let style = move || {
        let f = now().map_or(0.0, |l| mtr::fill(&l));
        if vertical {
            format!("height: {f:.1}%")
        } else {
            format!("width: {f:.1}%")
        }
    };
    let title = move || {
        now2().map_or_else(
            || "no level".to_owned(),
            |l| {
                if src2() {
                    format!(
                        "{} at the source (this device sends no meters)",
                        mtr::text(&l)
                    )
                } else {
                    format!("{} {}", l.label, mtr::text(&l))
                }
            },
        )
    };
    let class = {
        let from_source = from_source.clone();
        move || match (vertical, from_source()) {
            (true, _) => "lvl v",
            (false, true) => "lvl src",
            (false, false) => "lvl",
        }
    };
    view! {
        <span class=class data-testid="lvl" data-source=move || from_source().then_some("1")
            data-dir=if tx { "tx" } else { "rx" } title=title>
            <span class=move || now3().map_or("meter none", |l| mtr::class(&l)) style=style></span>
        </span>
    }
}

/// The meters view: pick a device, see its TX and RX channels' levels.
#[component]
fn MeterView(d: Devices) -> impl IntoView {
    let mtr = expect_context::<Mtr>();
    let names: Vec<String> = d
        .devices
        .iter()
        .filter(|dev| {
            dev.channels
                .as_ref()
                .is_some_and(|c| !c.tx.is_empty() || !c.rx.is_empty())
        })
        .map(|dev| dev.name.clone())
        .collect();
    // Every such device with its channel counts, for the page's first view.
    let all: Vec<(String, usize, usize)> = d
        .devices
        .iter()
        .filter_map(|dev| {
            let c = dev.channels.as_ref()?;
            (!c.tx.is_empty() || !c.rx.is_empty())
                .then(|| (dev.name.clone(), c.tx.len(), c.rx.len()))
        })
        .collect();
    view! {
        <div class="picker">
            {names.into_iter().map(|name| {
                let n = name.clone();
                let href = format!("#meters/{}", String::from(js_sys::encode_uri_component(&name)));
                let on = {
                    let n = name.clone();
                    move || mtr.device.get().as_deref() == Some(n.as_str())
                };
                view! {
                    <a href=href data-testid="mtr-device" data-name=name.clone()
                        class=move || if on() { "tab on" } else { "tab" }
                        on:click=move |_| {
                            mtr.report.set(None);
                            mtr.device.set(Some(n.clone()));
                        }>{name.clone()}</a>
                }
            }).collect_view()}
        </div>
        {move || match (mtr.device.get(), mtr.report.get()) {
            // No device chosen: every device's loudest channel each way, so
            // the page says at once who sends and who receives sound (#123).
            (None, _) => {
                let rows = all
                    .iter()
                    .map(|(name, tx, rx)| {
                        let href = format!("#meters/{}", String::from(js_sys::encode_uri_component(name)));
                        let n = name.clone();
                        view! {
                            <tr data-testid="mtr-all-row" data-name=name.clone()>
                                <td><a class="strong" href=href on:click=move |_| {
                                    mtr.report.set(None);
                                    mtr.device.set(Some(n.clone()));
                                }>{name.clone()}</a></td>
                                <td>{(*tx > 0).then(|| level_bar(name, true, None, false, Vec::new()))}</td>
                                <td class="muted">{format!("{tx} TX")}</td>
                                <td>{(*rx > 0).then(|| level_bar(name, false, None, false, Vec::new()))}</td>
                                <td class="muted">{format!("{rx} RX")}</td>
                            </tr>
                        }
                    })
                    .collect_view();
                view! {
                    <div class="table-wrap">
                    <table class="grid meters-all" data-testid="mtr-all">
                        <thead><tr><th>"Device"</th><th colspan="2">"Sends (loudest channel)"</th><th colspan="2">"Receives (loudest channel)"</th></tr></thead>
                        <tbody>{rows}</tbody>
                    </table>
                    </div>
                    <p class="muted small">"Every device's loudest channel now. A click on a device shows each of its channels."</p>
                }.into_any()
            }
            (Some(_), None) => view! { <p class="status" data-testid="mtr-wait">"Subscribing…"</p> }.into_any(),
            (Some(_), Some(Err(e))) => view! { <p class="status error">{format!("Not readable: {e}")}</p> }.into_any(),
            (Some(_), Some(Ok(r))) => meter_report_view(r),
        }}
    }
}

fn meter_column(title: &'static str, id: &'static str, levels: Vec<ChannelLevel>) -> impl IntoView {
    view! {
        <div class="meters" data-testid=id>
            <h3>{title}</h3>
            {levels.into_iter().map(|l| {
                let style = format!("width: {:.1}%", mtr::fill(&l));
                view! {
                    <div class="mrow" data-testid="mtr-channel">
                        <span class="mlabel">{l.label.clone()}</span>
                        <span class="mbar"><span class=mtr::class(&l) style=style></span></span>
                        <span class="mval" data-testid="mtr-value">{mtr::text(&l)}</span>
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

fn meter_report_view(r: MeterReport) -> AnyView {
    // The device's name leads to its place on the Map (#123), as on
    // every other page.
    let head = match r.age_ms {
        Some(a) => format!(" · {} frames · last {} ms ago", r.frames, a),
        None => " · waiting for the first meter frame".to_owned(),
    };
    let error = r
        .error
        .clone()
        .map(|e| view! { <p class="error" data-testid="mtr-error">{e}</p> });
    view! {
        <p class="status" data-testid="mtr-head">{pages::dev_link(&r.device)}{head}</p>
        {error}
        <div class="meter-cols">
            {meter_column("Transmit", "mtr-tx", r.tx)}
            {meter_column("Receive", "mtr-rx", r.rx)}
        </div>
    }
    .into_any()
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

fn name_text(n: &Option<String>) -> String {
    n.clone().unwrap_or_else(|| "default".to_owned())
}

/// [`change_target`] with the device's name a link to its place on the Map
/// (#123), as on every other page.
fn target_view(e: &ChangeEntry) -> AnyView {
    let text = change_target(e);
    match changes::device_of(e).and_then(|d| changes::around(&text, d).map(|p| (d, p))) {
        Some((d, (before, after))) => {
            view! { {before.to_owned()}{pages::dev_link(d)}{after.to_owned()} }.into_any()
        }
        None => text.into_any(),
    }
}

/// What a logged change changed, in words.
fn change_target(e: &ChangeEntry) -> String {
    match (e.kind.as_str(), e.what.as_str()) {
        ("rename", "device") => format!("rename device {}", e.rx_device),
        ("rename", "tx") => format!("rename TX {} @ {}", e.rx_channel, e.rx_device),
        ("rename", _) => format!("rename RX {} @ {}", e.rx_channel, e.rx_device),
        ("setting", w) => format!("{} @ {}", w.replace('_', " "), e.rx_device),
        ("multicast", "delete") => format!(
            "delete multicast flow slot {} @ {}",
            e.rx_channel, e.rx_device
        ),
        ("multicast", _) => format!("new multicast flow @ {}", e.rx_device),
        ("status", "mdns") => format!("mDNS sender {} ({})", e.rx_label, e.rx_device),
        ("status", "mdns-browser") => {
            format!("96 s browser {} ({})", e.rx_label, e.rx_device)
        }
        ("status", "network-floor") => format!("dstack {} at {}", e.rx_device, e.rx_label),
        ("status", _) => format!(
            "{} @ {} \u{2190} {}",
            e.rx_label,
            e.rx_device,
            source_text(&e.after)
        ),
        _ => format!("{} @ {}", e.rx_label, e.rx_device),
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
    renaming: RwSignal<Option<RenameAsk>>,
    setting: RwSignal<Option<settings::Ask>>,
    /// A new multicast flow waiting in its dialog.
    multicast: RwSignal<Option<McAsk>>,
    /// A multicast flow delete waiting in its confirm dialog.
    mc_delete: RwSignal<Option<McDelete>>,
    /// The devices whose TX flows are open, with their last list (kept
    /// across the device poll's rebuilds).
    flows: RwSignal<BTreeMap<String, Option<Result<TxFlows, String>>>>,
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

/// A rename the user asked for, waiting in the rename dialog.
#[derive(Clone, Debug, PartialEq)]
struct RenameAsk {
    device: String,
    target: RenameTarget,
    current: String,
    /// The subscriptions that name what is renamed (they stop until changed).
    affected: Vec<String>,
}

impl RenameAsk {
    /// A rename of TX channel `channel` (`label`) of `device`.
    fn tx(d: &Devices, device: &str, channel: u16, label: &str) -> Self {
        RenameAsk {
            device: device.to_owned(),
            target: RenameTarget::Tx(channel),
            current: label.to_owned(),
            affected: affected(d, device, Some(label)),
        }
    }

    fn rx(device: &str, channel: u16, label: &str) -> Self {
        RenameAsk {
            device: device.to_owned(),
            target: RenameTarget::Rx(channel),
            current: label.to_owned(),
            affected: Vec::new(),
        }
    }

    fn device(d: &Devices, device: &str) -> Self {
        RenameAsk {
            device: device.to_owned(),
            target: RenameTarget::Device,
            current: device.to_owned(),
            affected: affected(d, device, None),
        }
    }

    fn title(&self) -> String {
        match self.target {
            RenameTarget::Tx(c) => format!(
                "Rename TX channel {c} \u{201c}{}\u{201d} of {}",
                self.current, self.device
            ),
            RenameTarget::Rx(c) => format!(
                "Rename RX channel {c} \u{201c}{}\u{201d} of {}",
                self.current, self.device
            ),
            RenameTarget::Device => format!("Rename device {}", self.device),
        }
    }
}

async fn send_rename(req: &Rename) -> Result<ChangeResult, String> {
    let res = gloo_net::http::Request::post("/api/rename")
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
fn RenameDialog() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let name = RwSignal::new(String::new());
    // A new ask fills the input with the current name.
    Effect::new(move |_| {
        if let Some(a) = ctl.renaming.get() {
            name.set(a.current.clone());
        }
    });
    let send = move |new: Option<String>| {
        let Some(a) = ctl.renaming.get_untracked() else {
            return;
        };
        ctl.busy.set(true);
        spawn_local(async move {
            let req = Rename {
                device: a.device.clone(),
                target: a.target,
                name: new,
            };
            let r = send_rename(&req).await.unwrap_or_else(|e| ChangeResult {
                ok: false,
                message: e,
                channel: None,
            });
            ctl.result.set(Some(r));
            ctl.devices.set(Some(fetch_devices().await));
            ctl.busy.set(false);
            ctl.renaming.set(None);
        });
    };
    move || {
        ctl.renaming.get().map(|a| {
            let busy = ctl.busy.get();
            let channel = !matches!(a.target, RenameTarget::Device);
            let warn = (!a.affected.is_empty()).then(|| {
                view! {
                    <p class="warn" data-testid="rename-affected">
                        {format!(
                            "{} subscription(s) use this name and stop until they are changed: {}",
                            a.affected.len(),
                            a.affected.join(", ")
                        )}
                    </p>
                }
            });
            view! {
                <div class="modal-back">
                    <div class="modal" role="dialog" data-testid="rename-dialog">
                        <p data-testid="rename-title">{a.title()}</p>
                        <input data-testid="rename-input" maxlength="31" prop:value=move || name.get()
                            on:input=move |ev| name.set(event_target_value(&ev)) />
                        {warn}
                        <div class="buttons">
                            <button data-testid="rename-cancel" disabled=busy
                                on:click=move |_| ctl.renaming.set(None)>"Cancel"</button>
                            {channel.then(|| view! {
                                <button data-testid="rename-reset" disabled=busy
                                    on:click=move |_| send(None)>"Reset to default"</button>
                            })}
                            <button class="primary" data-testid="rename-confirm"
                                disabled=move || busy || name.get().trim().is_empty() || name.get() == a.current
                                on:click=move |_| send(Some(name.get_untracked().trim().to_owned()))>
                                {if busy { "Sending…" } else { "Rename" }}
                            </button>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

async fn send_setting(req: &SettingChange) -> Result<ChangeResult, String> {
    let res = gloo_net::http::Request::post("/api/setting")
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

/// The setting dialog: choose a value, read what it does, Apply sends it.
#[component]
fn SettingDialog() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let value = RwSignal::new(None::<u32>);
    // The device's largest packet lateness in the last hour, for the
    // latency dialog's note.
    let peak = RwSignal::new(None::<u32>);
    Effect::new(move |_| {
        peak.set(None);
        if let Some(a) = ctl.setting.get() {
            value.set(a.current);
            if a.kind == settings::Kind::Latency {
                spawn_local(async move {
                    if let Ok(o) =
                        pages::get_json::<dcontroller_api::Overview>("/api/overview").await
                    {
                        peak.set(
                            o.receivers
                                .iter()
                                .find(|r| r.device.eq_ignore_ascii_case(&a.device))
                                .map(|r| r.peak_us),
                        );
                    }
                });
            }
        }
    });
    let apply = move |_| {
        let (Some(a), Some(v)) = (ctl.setting.get_untracked(), value.get_untracked()) else {
            return;
        };
        ctl.busy.set(true);
        spawn_local(async move {
            let req = SettingChange {
                device: a.device.clone(),
                setting: a.kind.setting(v),
            };
            let r = send_setting(&req).await.unwrap_or_else(|e| ChangeResult {
                ok: false,
                message: e,
                channel: None,
            });
            ctl.result.set(Some(r));
            ctl.devices.set(Some(fetch_devices().await));
            ctl.busy.set(false);
            ctl.setting.set(None);
        });
    };
    move || {
        ctl.setting.get().map(|a| {
            let busy = ctl.busy.get();
            let kind = a.kind;
            let current = a.current;
            view! {
                <div class="modal-back">
                    <div class="modal" role="dialog" data-testid="setting-dialog">
                        <p data-testid="setting-title">{a.title()}</p>
                        <select data-testid="setting-value"
                            on:change=move |ev| value.set(event_target_value(&ev).parse().ok())>
                            {a.options.iter().map(|&o| view! {
                                <option value=o.to_string() selected=move || value.get() == Some(o)>{kind.text(o)}</option>
                            }).collect_view()}
                        </select>
                        <p class="warn" data-testid="setting-warning">{kind.warning()}</p>
                        {move || value.get().filter(|_| kind == settings::Kind::Latency)
                            .and_then(|v| settings::peak_note(v, peak.get()))
                            .map(|n| view! { <p class="warn" data-testid="setting-peak">{n}</p> })}
                        <div class="buttons">
                            <button data-testid="setting-cancel" disabled=busy
                                on:click=move |_| ctl.setting.set(None)>"Cancel"</button>
                            <button class="primary" data-testid="setting-apply"
                                disabled=move || busy || value.get().is_none() || value.get() == current
                                on:click=apply>
                                {move || if busy { "Sending…".to_owned() } else {
                                    value.get().map_or("Apply".to_owned(), |v| format!("Set {}", kind.text(v)))
                                }}
                            </button>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

async fn fetch_flows(device: &str) -> Result<TxFlows, String> {
    let url = format!("/api/flows/{}", js_sys::encode_uri_component(device));
    let res = gloo_net::http::Request::get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        let why = res.text().await.unwrap_or_default();
        return Err(format!("server answered {}: {why}", res.status()));
    }
    res.json::<TxFlows>().await.map_err(|e| e.to_string())
}

/// Lists `device`'s TX flows into the open set.
fn load_flows(ctl: Ctl, device: String) {
    ctl.flows.update(|m| {
        m.insert(device.clone(), None);
    });
    spawn_local(async move {
        let r = fetch_flows(&device).await;
        ctl.flows.update(|m| {
            if m.contains_key(&device) {
                m.insert(device, Some(r));
            }
        });
    });
}

/// A device's TX flows, listed when the operator opens them, and the
/// button that asks for a new multicast flow.
#[component]
fn TxFlowsBox(device: String, tx: Vec<(u16, String)>) -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let key = device.clone();
    let open = move || ctl.flows.with(|m| m.contains_key(&key));
    let toggle = {
        let device = device.clone();
        move |_| {
            if ctl.flows.with_untracked(|m| m.contains_key(&device)) {
                ctl.flows.update(|m| {
                    m.remove(&device);
                });
            } else {
                load_flows(ctl, device.clone());
            }
        }
    };
    let ask = {
        let device = device.clone();
        let tx = tx.clone();
        move |_| {
            ctl.multicast.set(Some(McAsk {
                device: device.clone(),
                tx: tx.clone(),
                chosen: Vec::new(),
            }))
        }
    };
    let list = {
        let device = device.clone();
        move || {
            let state = ctl.flows.with(|m| m.get(&device).cloned());
            match state {
                None => ().into_any(),
                Some(None) => view! { <p class="status" data-testid="flows-loading">"Reading the flows…"</p> }.into_any(),
                Some(Some(Err(e))) => view! { <p class="error" data-testid="flows-error">{format!("Flows not readable: {e}")}</p> }.into_any(),
                Some(Some(Ok(f))) => {
                    let all = ctl.devices.with_untracked(|d| match d {
                        Some(Ok(d)) => flows::rows(&f.flows, &tx, d),
                        _ => flows::rows(&f.flows, &tx, &Devices::default()),
                    });
                    let (count, full) = flows::count_text(all.len(), f.slots, f.unread);
                    view! {
                        <p class={if full { "subs warn" } else { "subs" }} data-testid="flows-count">{count}</p>
                        <table class="grid" data-testid="flows-table">
                            <thead><tr><th>"Slot"</th><th>"Kind"</th><th>"Channels"</th><th>"To"</th><th></th></tr></thead>
                            <tbody>{all.into_iter().map(|r| view! {
                                <tr data-testid="flow-row" data-kind=if r.multicast { "multicast" } else { "unicast" }>
                                    <td>{r.slot}</td>
                                    <td>{if r.multicast { "multicast" } else { "unicast" }}</td>
                                    <td data-testid="flow-channels">{r.channels.clone()}</td>
                                    <td data-testid="flow-to">{r.to.clone()}</td>
                                    <td>{r.multicast.then(|| {
                                        let ask = McDelete {
                                            device: f.device.clone(),
                                            slot: r.slot,
                                            text: flows::delete_question(&f.device, &r),
                                        };
                                        view! {
                                            <button data-testid="flow-delete"
                                                on:click=move |_| ctl.mc_delete.set(Some(ask.clone()))>"Delete"</button>
                                        }
                                    })}</td>
                                </tr>
                            }).collect_view()}</tbody>
                        </table>
                    }.into_any()
                }
            }
        }
    };
    view! {
        <div class="actions">
            <button data-testid="tx-flows" on:click=toggle>{move || if open() { "Hide TX flows" } else { "TX flows" }}</button>
            <button data-testid="create-multicast" on:click=ask>"Create multicast flow"</button>
        </div>
        {list}
    }
}

/// A new multicast flow the operator is choosing channels for.
#[derive(Clone, Debug, PartialEq)]
struct McAsk {
    device: String,
    tx: Vec<(u16, String)>,
    chosen: Vec<u16>,
}

/// A multicast flow delete the operator asked for.
#[derive(Clone, Debug, PartialEq)]
struct McDelete {
    device: String,
    slot: u16,
    text: String,
}

#[component]
fn MulticastDeleteDialog() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let confirm = move |_| {
        let Some(a) = ctl.mc_delete.get_untracked() else {
            return;
        };
        ctl.busy.set(true);
        spawn_local(async move {
            let req = MulticastDelete {
                device: a.device.clone(),
                slot: a.slot,
            };
            let r = post_change("/api/multicast/delete", &req)
                .await
                .unwrap_or_else(|e| ChangeResult {
                    ok: false,
                    message: e,
                    channel: None,
                });
            ctl.result.set(Some(r));
            load_flows(ctl, a.device.clone());
            ctl.busy.set(false);
            ctl.mc_delete.set(None);
        });
    };
    move || {
        ctl.mc_delete.get().map(|a| {
            let busy = ctl.busy.get();
            view! {
                <div class="modal-back">
                    <div class="modal" role="dialog" data-testid="flow-delete-dialog">
                        <p class="warn" data-testid="flow-delete-question">{a.text.clone()}</p>
                        <div class="buttons">
                            <button data-testid="flow-delete-cancel" disabled=busy
                                on:click=move |_| ctl.mc_delete.set(None)>"Cancel"</button>
                            <button class="primary" data-testid="flow-delete-confirm" disabled=busy
                                on:click=confirm>{if busy { "Sending…" } else { "Delete" }}</button>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

async fn post_change<T: serde::Serialize>(url: &str, req: &T) -> Result<ChangeResult, String> {
    let res = gloo_net::http::Request::post(url)
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

async fn send_multicast(req: &MulticastCreate) -> Result<ChangeResult, String> {
    let res = gloo_net::http::Request::post("/api/multicast")
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
fn MulticastDialog() -> impl IntoView {
    let ctl = expect_context::<Ctl>();
    let create = move |_| {
        let Some(a) = ctl.multicast.get_untracked() else {
            return;
        };
        ctl.busy.set(true);
        spawn_local(async move {
            let req = MulticastCreate {
                device: a.device.clone(),
                channels: a.chosen.clone(),
            };
            let r = send_multicast(&req).await.unwrap_or_else(|e| ChangeResult {
                ok: false,
                message: e,
                channel: None,
            });
            ctl.result.set(Some(r));
            load_flows(ctl, a.device.clone());
            ctl.busy.set(false);
            ctl.multicast.set(None);
        });
    };
    move || {
        ctl.multicast.get().map(|a| {
            let busy = ctl.busy.get();
            let full = a.chosen.len() >= flows::MAX_CHANNELS;
            view! {
                <div class="modal-back">
                    <div class="modal" role="dialog" data-testid="multicast-dialog">
                        <p>{format!("New multicast flow on {}: choose up to {} TX channels.", a.device, flows::MAX_CHANNELS)}</p>
                        <div class="checks">
                            {a.tx.iter().map(|(n, l)| {
                                let n = *n;
                                let on = a.chosen.contains(&n);
                                view! {
                                    <label data-testid="multicast-channel" data-channel=n.to_string()>
                                        <input type="checkbox" checked=on disabled=busy || (full && !on)
                                            on:change=move |_| ctl.multicast.update(|m| if let Some(m) = m {
                                                if let Some(i) = m.chosen.iter().position(|c| *c == n) {
                                                    m.chosen.remove(i);
                                                } else {
                                                    m.chosen.push(n);
                                                    m.chosen.sort_unstable();
                                                }
                                            })/>
                                        {format!(" {l} ({n})")}
                                    </label>
                                }
                            }).collect_view()}
                        </div>
                        {(!a.chosen.is_empty()).then(|| view! {
                            <p class="warn" data-testid="multicast-question">{flows::question(&a.device, &a.chosen, &a.tx)}</p>
                        })}
                        <div class="buttons">
                            <button data-testid="multicast-cancel" disabled=busy
                                on:click=move |_| ctl.multicast.set(None)>"Cancel"</button>
                            <button class="primary" data-testid="multicast-create"
                                disabled=busy || a.chosen.is_empty() on:click=create>
                                {if busy { "Sending…" } else { "Create" }}
                            </button>
                        </div>
                    </div>
                </div>
            }
        })
    }
}

/// What the address opens, as the record of use names it: the view and
/// the rest of the hash (#142).
fn use_of(hash: &str) -> dcontroller_api::UseEvent {
    let (view, target) = changes::use_parts(hash);
    let target = js_sys::decode_uri_component(&target)
        .ok()
        .map(String::from)
        .unwrap_or(target);
    dcontroller_api::UseEvent { view, target }
}

/// Tells the server what was just opened; a failure costs nothing but
/// the record, so it is only logged.
fn record_use() {
    let e = use_of(&window().location().hash().unwrap_or_default());
    spawn_local(async move {
        let sent = match gloo_net::http::Request::post("/api/usage").json(&e) {
            Ok(r) => r.send().await.map_err(|e| e.to_string()),
            Err(err) => Err(err.to_string()),
        };
        match sent {
            Ok(r) if r.ok() => {}
            Ok(r) => leptos::logging::log!("usage: server answered {}", r.status()),
            Err(err) => leptos::logging::log!("usage: not sent: {err}"),
        }
    });
}

/// How often each view and thing was opened (#142): what the design
/// should serve first.
#[component]
fn UsageTable(usage: RwSignal<Option<Result<dcontroller_api::Usage, String>>>) -> impl IntoView {
    move || {
        match usage.get() {
        None => view! { <p class="status">"Reading…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="status error">{format!("Use not readable: {e}")}</p> }.into_any(),
        Some(Ok(u)) if u.rows.is_empty() => {
            view! { <p class="status" data-testid="usage-empty">"Nothing opened yet."</p> }.into_any()
        }
        Some(Ok(u)) => view! {
            <p class="status" data-testid="usage-summary">{format!("What was opened in the app in the last {} days, the most first.", u.days)}</p>
            <div class="table-wrap">
            <table class="grid" data-testid="usage-table">
                <thead><tr><th>"Page"</th><th>"What"</th><th class="num">"Times"</th><th>"Last"</th></tr></thead>
                <tbody>
                    {u.rows.into_iter().map(|r| {
                        let href = if r.target.is_empty() { format!("#{}", r.view) } else { format!("#{}/{}", r.view, r.target) };
                        view! {
                            <tr data-testid="usage-row">
                                <td>{r.view.clone()}</td>
                                <td><a href=href>{if r.target.is_empty() { "the page".to_owned() } else { r.target.clone() }}</a></td>
                                <td class="num">{r.count}</td>
                                <td>{pages::time_text(r.last)}</td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
            </div>
        }.into_any(),
    }
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
/// One logged change or network alarm, as its own row.
fn change_row(e: &ChangeEntry) -> AnyView {
    let when = pages::time_text(e.at as i64);
    let names = matches!(
        e.kind.as_str(),
        "rename" | "setting" | "multicast" | "status"
    );
    let cell = |n: &Option<String>, s: &Option<Source>| {
        if names { name_text(n) } else { source_text(s) }
    };
    let class = if !e.ok {
        "err"
    } else if e.user == "outside dcontroller" {
        "ext"
    } else {
        ""
    };
    let who = if e.user == "outside dcontroller" || e.user == "device status" {
        view! { <span class="chip info">{e.user.clone()}</span> }.into_any()
    } else {
        e.user.clone().into_any()
    };
    view! {
        <tr data-testid="change-row" data-kind=e.kind.clone() class=class>
            <td>{when}</td>
            <td>{who}</td>
            <td>{target_view(e)}</td>
            <td>{cell(&e.name_before, &e.before)}</td>
            <td>{cell(&e.name_asked, &e.requested)}</td>
            <td>{cell(&e.name_after, &e.after)}</td>
            <td>{e.message.clone()}</td>
        </tr>
    }
    .into_any()
}

/// One connection's outage as one row (#123): from its stop to its return.
fn outage_row(o: &changes::Outage) -> AnyView {
    let time = |t: u64| pages::time_text(t as i64);
    let when = time(o.start.or(o.end).unwrap_or(0));
    let link = map::Sel::Link {
        from: o.from.clone(),
        to: o.to.clone(),
    }
    .hash();
    let why = o.reasons.join(", ");
    let (detail, result) = match (o.start, o.end) {
        (Some(a), Some(b)) => (
            // The end's date only when it is another day.
            format!("not receiving {} – {}", time(a), {
                let (ta, tb) = (time(a), time(b));
                match (ta.rsplit_once(' '), tb.rsplit_once(' ')) {
                    (Some((da, _)), Some((db, clock))) if da == db => clock.to_owned(),
                    _ => tb,
                }
            }),
            format!(
                "received again after {} · was: {why}",
                changes::span(o.seconds().unwrap_or(0))
            ),
        ),
        (Some(a), None) => (
            format!("not receiving since {}", time(a)),
            format!("stopped receiving: {why}"),
        ),
        (None, _) => (
            "its stop is no longer in the log".to_owned(),
            "receiving again".to_owned(),
        ),
    };
    let open = o.end.is_none();
    view! {
        <tr data-testid="change-row" data-kind="status" data-outage=if open { "open" } else { "over" }
            class=if open { "err" } else { "" }>
            <td>{when}</td>
            <td><span class="chip info">"device status"</span></td>
            <td>
                <a class="strong" href=link>{format!("{} \u{2192} {}", o.from, o.to)}</a>
                <br/><span class="muted small">{o.channels.join(", ")}</span>
            </td>
            <td colspan="3">{detail}</td>
            <td>{result}</td>
        </tr>
    }
    .into_any()
}

/// How many rows the Changes page shows before "show older".
const CHANGES_PAGE: usize = 100;

#[component]
fn ChangeLog() -> impl IntoView {
    let list = RwSignal::new(None::<Result<Vec<ChangeEntry>, String>>);
    spawn_local(async move { list.set(Some(fetch_changes().await)) });
    // 0: everything, 1: changes made, 2: what devices did, 3: what is
    // opened in the app (#142).
    let filter = RwSignal::new(0u8);
    let usage = RwSignal::new(None::<Result<dcontroller_api::Usage, String>>);
    Effect::new(move |_| {
        if filter.get() == 3 {
            spawn_local(async move { usage.set(Some(pages::get_json("/api/usage").await)) });
        }
    });
    let limit = RwSignal::new(CHANGES_PAGE);
    // `#changes/<device>`: only what concerns that device (a connection's
    // "Changes of" link on the Map, #142); the hash listener keeps it.
    let ChangesOnly(only) = expect_context::<ChangesOnly>();
    move || {
        let only = only.get();
        match list.get() {
        None => view! { <p class="status">"Loading…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="status error">{format!("Changes not readable: {e}")}</p> }.into_any(),
        Some(Ok(v)) if v.is_empty() => view! { <p class="status" data-testid="changes-empty">"No changes yet. Changes made here, and changes seen on the network (made in Dante Controller, on a mixer or a device itself), show up in this list."</p> }.into_any(),
        Some(Ok(v)) => {
            let mut items = changes::fold(&v);
            if let Some(name) = &only {
                items.retain(|i| i.concerns(name));
            }
            let made = items.iter().filter(|i| !i.is_status()).count();
            let status = items.len() - made;
            let f = filter.get();
            let visible: Vec<&changes::Item> = items
                .iter()
                .filter(|i| match f {
                    1 => !i.is_status(),
                    2 => i.is_status(),
                    _ => true,
                })
                .collect();
            let (total, max) = (visible.len(), limit.get());
            let rows = visible
                .into_iter()
                .take(max)
                .map(|i| match i {
                    changes::Item::Change(e) | changes::Item::Alarm(e) => change_row(e),
                    changes::Item::Outage(o) => outage_row(o),
                })
                .collect_view();
            let seg = move |k: u8, label: String, id: &'static str| {
                view! {
                    <button data-testid=id class=move || if filter.get() == k { "seg on" } else { "seg" }
                        on:click=move |_| { limit.set(CHANGES_PAGE); filter.set(k); }>{label}</button>
                }
            };
            let older = (total > max).then(|| view! {
                <button class="act" data-testid="changes-older" on:click=move |_| limit.update(|l| *l += CHANGES_PAGE)>
                    {format!("Show older ({} more)", total - max)}
                </button>
            });
            view! {
                <div class="bar">
                    <div class="seg-group" data-testid="changes-filter">
                        {seg(0, format!("All {}", items.len()), "changes-all")}
                        {seg(1, format!("Changes made {made}"), "changes-made")}
                        {seg(2, format!("Device status {status}"), "changes-status")}
                        {seg(3, "What is opened".to_owned(), "changes-usage")}
                    </div>
                    {only.clone().map(|name| view! {
                        <span class="chip info" data-testid="changes-only">{format!("only {name}")}</span>
                        <a class="act" href="#changes" data-testid="changes-every">"every device"</a>
                    })}
                    <span class="muted small">"A connection that stopped and received again is one line, with how long it took."</span>
                </div>
                {(f == 3).then(|| view! { <UsageTable usage=usage/> })}
                <div class="table-wrap" style=if f == 3 { "display:none" } else { "" }>
                <table class="changes" data-testid="changes-table">
                    <thead><tr><th>"When"</th><th>"Who"</th><th>"What"</th><th>"Before"</th><th>"Asked"</th><th>"After"</th><th>"Result"</th></tr></thead>
                    <tbody>{rows}</tbody>
                </table>
                </div>
                {older}
            }.into_any()
        }
    }
    }
}

/// The expanded TX and RX devices of the matrix, kept across refreshes.
#[derive(Clone, Copy)]
struct Expanded {
    tx: RwSignal<BTreeSet<String>>,
    rx: RwSignal<BTreeSet<String>>,
    /// The matrix box's scroll (top, left), kept across rebuilds.
    scroll: StoredValue<(i32, i32)>,
    /// The subscriptions line above the matrix is open, kept across rebuilds.
    problems: StoredValue<bool>,
    /// The hovered cross-point's row and column highlight (a CSS rule).
    hover: RwSignal<String>,
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
    // One line, only when a subscription to a device on the network does
    // not receive; a switched-off source is no error. Details on a click.
    let trouble = problems(d);
    let (failing, unresolved, offline) = tally(&trouble);
    let trouble = (failing + unresolved > 0).then(|| {
        let mut parts = Vec::new();
        if failing > 0 {
            parts.push(view! { <span class="t-err" data-testid="sub-failing">{format!("{failing} failing")}</span> }.into_any());
        }
        if unresolved > 0 {
            parts.push(view! { <span class="t-warn" data-testid="sub-unresolved">{format!("{unresolved} not resolved")}</span> }.into_any());
        }
        if offline > 0 {
            parts.push(view! { <span class="t-off" data-testid="sub-offline">{format!("{offline} from devices offline")}</span> }.into_any());
        }
        let rows = trouble
            .into_iter()
            .filter(|p| p.kind != matrix::Kind::Offline)
            .map(|p| {
                let key = p.rx.clone();
                let text = format!("{} \u{2190} {}", p.rx, p.source);
                view! {
                    <li class=p.kind.class() data-testid="sub-problem" data-rx=key>
                        <span class="p-what">{text}</span>
                        <span class="p-status">{p.status}</span>
                    </li>
                }
            })
            .collect_view();
        let box_class = if failing > 0 { "problems err" } else { "problems warn" };
        let is_open = ex.problems.get_value();
        let toggle = move |ev: leptos::ev::Event| {
            ex.problems
                .set_value(event_target::<web_sys::HtmlDetailsElement>(&ev).open());
        };
        view! {
            <details class=box_class data-testid="sub-problems" open=is_open on:toggle=toggle>
                <summary data-testid="sub-summary">"Subscriptions: "{parts}</summary>
                <ul>{rows}</ul>
            </details>
        }
    });
    let grid = Grid::of(d);
    let Expanded {
        tx: tx_open,
        rx: rx_open,
        scroll,
        hover,
        ..
    } = ex;
    let ctl = expect_context::<Ctl>();
    let states = expect_context::<LinkStates>().0.get();
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
            // A folded device is one narrow column, its name written up the
            // header's height (#123: with names across, six of fifteen
            // devices filled the page's width).
            let bar = (!open).then(|| level_bar(&t.device, true, None, true, Vec::new()));
            view! {
                <th class=if open { "txdev open" } else { "txdev folded" } colspan=span
                    rowspan=if open { 1 } else { 2 }
                    data-testid="tx-device" data-name=name_attr title=t.device.clone()
                    on:click=move |_| flip(tx_open, &name)>
                    <span>{label}</span>
                    <NoMeters device=t.device.clone()/>
                    {bar}
                </th>
            }
        })
        .collect_view();
    let head_channels = cols
        .iter()
        .map(|&(i, c)| match c {
            Some(c) => {
                let t = &grid.tx[i];
                let ask = RenameAsk::tx(d, &t.device, t.numbers[c], &t.channels[c]);
                view! {
                    <th class="txch rename" data-testid="tx-channel-head"
                        data-name=format!("{}/{}", t.device, t.channels[c])
                        title="Rename this TX channel"
                        on:click=move |_| ctl.renaming.set(Some(ask.clone()))>
                        <span>{t.channels[c].clone()}</span>
                        {level_bar(&t.device, true, Some(t.numbers[c]), true, Vec::new())}
                    </th>
                }
                .into_any()
            }
            // A folded device's header spans both rows.
            None => ().into_any(),
        })
        .collect_view();
    let mut rows = Vec::new();
    for r in &grid.rx {
        let open = rxo.contains(&r.device);
        let name = r.device.clone();
        let label = r.device.clone();
        let name_attr = r.device.clone();
        let srcs: Vec<(String, u16)> = r
            .channels
            .iter()
            .filter_map(|rc| matrix::source_of(&grid, rc))
            .collect();
        let bar = (!open).then(|| level_bar(&r.device, false, None, false, srcs));
        // The receiver's subscriptions that do not receive from a device on
        // the network: red when Dante reports an error, amber otherwise.
        let badge = match matrix::trouble(d, r) {
            (0, 0) => None,
            (f, u) => Some(view! {
                <span class={if f > 0 { "badge err" } else { "badge warn" }} data-testid="rx-trouble"
                    title=format!("{f} failing, {u} not resolved")>{(f + u).to_string()}</span>
            }),
        };
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
                // Device by device, the cell is the connection: the worse
                // of its status now and of its last hour, and a link to
                // it on the Map. One of a transmitter's channels is not a
                // connection of its own.
                let link =
                    (ch.is_none() && n > 0).then(|| matrix::link_of(&states, &t.device, &r.device));
                let (class, words) = match link {
                    Some(l) => matrix::point(x, l),
                    None => (x.class(), String::new()),
                };
                let state = link.map(|l| l.unwrap_or(map::State::Unknown).class());
                let href = link.map(|_| {
                    map::Sel::Link {
                        from: t.device.clone(),
                        to: r.device.clone(),
                    }
                    .hash()
                });
                view! {
                    <td class=class data-testid="cell" data-rx=r.device.clone() data-tx=tx_attr
                        data-link=state title=words>
                        {match href {
                            Some(href) => view! { <a href=href>{n.to_string()}</a> }.into_any(),
                            None => view! { {(n > 0).then(|| n.to_string())} }.into_any(),
                        }}
                    </td>
                }
            })
            .collect_view();
        rows.push(view! {
            <tr class="rxdev">
                <th class=if open { "rxname open" } else { "rxname" } data-testid="rx-device" data-name=name_attr
                    on:click=move |_| flip(rx_open, &name)>
                    <span>{label}</span>
                    <NoMeters device=r.device.clone()/>
                    {badge}
                    {bar}
                </th>
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
                            <th class="rxchname rename" data-testid="rx-channel-head"
                                data-name=format!("{}/{}", r.device, rc.label)
                                title="Rename this RX channel"
                                on:click={
                                    let ask = RenameAsk::rx(&r.device, rc.channel, &rc.label);
                                    move |_| ctl.renaming.set(Some(ask.clone()))
                                }>
                                <span>{rc.label.clone()}</span>
                                {level_bar(&r.device, false, Some(rc.channel), false, matrix::source_of(&grid, rc).into_iter().collect())}
                            </th>
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
    // A rebuilt matrix opens where the last one was scrolled to, once laid out.
    let wrap = NodeRef::<leptos::html::Div>::new();
    wrap.on_load(move |el| {
        request_animation_frame(move || {
            let (top, left) = scroll.get_value();
            el.set_scroll_top(top);
            el.set_scroll_left(left);
        });
    });
    view! {
        <p class="status" data-testid="status">{line}</p>
        {trouble}
        <div class="matrix-wrap" node_ref=wrap
            on:scroll=move |ev| {
                let el = event_target::<web_sys::HtmlElement>(&ev);
                scroll.set_value((el.scroll_top(), el.scroll_left()));
            }>
            <style data-testid="crosshair">{move || hover.get()}</style>
            <table class="matrix" data-testid="matrix"
                on:mouseover=move |ev| {
                    let cell = ev
                        .target()
                        .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok())
                        .and_then(|e| e.closest("td.cell").ok().flatten());
                    let css = cell.and_then(|c| Some(matrix::crosshair(&c.get_attribute("data-rx")?, &c.get_attribute("data-tx")?)));
                    hover.set(css.unwrap_or_default());
                }
                on:mouseleave=move |_| hover.set(String::new())>
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

/// The Map's device table lists the devices newest first (else by name), kept
/// across the device poll's rebuilds.
#[derive(Clone, Copy)]
struct NewestFirst(RwSignal<bool>);

/// The short settings line: sample rate, encoding and active latency.
fn settings_line(s: &Settings, i: Option<&Info>) -> Option<String> {
    let rate = s
        .sample_rate
        .map(|r| format!("{} kHz", f64::from(r) / 1000.0));
    let bits = i.and_then(|i| i.encoding_bits).map(|b| format!("{b} bit"));
    let lat = s
        .latency_active_us
        .or(s.latency_configured_us)
        .map(|l| format!("latency {}", lat::ms(l)));
    let parts: Vec<String> = [rate, bits, lat].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The settings and device info in full, one `(name, value)` per row; only
/// what the device reports.
fn settings_rows(s: &Settings, i: Option<&Info>) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if let Some(r) = s.sample_rate {
        rows.push(("Sample rate", format!("{r} Hz")));
    }
    if let Some(b) = i.and_then(|i| i.encoding_bits) {
        rows.push(("Encoding", format!("PCM {b} bit")));
    }
    if let Some(p) = s.pullup {
        rows.push((
            "Pull-up",
            if p == 0 {
                "none".to_owned()
            } else {
                p.to_string()
            },
        ));
    }
    let opt = |v: Option<u32>| v.map(lat::ms);
    for (name, v) in [
        ("Latency, active", opt(s.latency_active_us)),
        ("Latency, configured", opt(s.latency_configured_us)),
        ("Latency, default", opt(s.latency_default_us)),
    ] {
        if let Some(v) = v {
            rows.push((name, v));
        }
    }
    if let (Some(lo), Some(hi)) = (s.latency_min_us, s.latency_max_us) {
        rows.push((
            "Latency, range",
            format!("{} – {}", lat::ms(lo), lat::ms(hi)),
        ));
    }
    if let Some(i) = i {
        for (name, v) in [
            ("Manufacturer", &i.manufacturer),
            ("Model", &i.model),
            ("Product version", &i.product_version),
            ("Software", &i.software_version),
            ("Firmware", &i.firmware_version),
            ("Dante platform", &i.platform),
            ("Dante firmware", &i.platform_software),
            ("Dante hardware", &i.platform_hardware),
        ] {
            if let Some(v) = v {
                rows.push((name, v.clone()));
            }
        }
    }
    rows
}

/// The clock rows of the device table.
fn clock_rows(c: &dcontroller_api::Clock) -> Vec<(&'static str, String)> {
    let mut rows = vec![
        ("Clock role", clock::role(c).to_owned()),
        ("Clock sync", c.sync.clone()),
        ("Frequency offset", clock::ppb(c.frequency_ppb)),
    ];
    if !c.leader {
        rows.push(("Clock master", c.master.clone()));
    }
    rows
}

/// The network rows of the device table: one per interface.
fn network_rows(ports: &[dcontroller_api::NetPort]) -> Vec<(&'static str, String)> {
    ports
        .iter()
        .take(2)
        .zip(["Primary network", "Secondary network"])
        .map(|(p, name)| (name, net::port_text(p)))
        .collect()
}

/// One part of a page (#123): its state for the strip on top, and its body.
struct Part {
    id: &'static str,
    title: &'static str,
    /// `ok`, `warn`, `bad`, or `none` (nothing to say yet).
    state: &'static str,
    body: AnyView,
}

/// A page of parts: a chip per part on top (a click scrolls to it), then
/// the parts, the ones in trouble first.
fn parts_view(parts: Vec<Part>) -> AnyView {
    let states: Vec<&str> = parts.iter().map(|p| p.state).collect();
    let order = parts::order(&states);
    let chips = parts
        .iter()
        .map(|p| {
            let id = p.id;
            view! {
                <button class=format!("chip {}", p.state) data-testid="part-chip" data-part=id data-state=p.state
                    title=format!("Show: {}", p.title)
                    on:click=move |_| {
                        if let Some(e) = document().get_element_by_id(&format!("part-{id}")) {
                            e.scroll_into_view();
                        }
                    }>
                    {p.title}<span class="chip-state">{parts::word(p.state)}</span>
                </button>
            }
        })
        .collect_view();
    let mut slots: Vec<Option<Part>> = parts.into_iter().map(Some).collect();
    let sections = order
        .into_iter()
        .filter_map(|i| slots[i].take())
        .map(|p| {
            view! {
                <section class="part" id=format!("part-{}", p.id) data-testid="part" data-part=p.id data-state=p.state>
                    <h2 class="sub">{p.title}</h2>
                    {p.body}
                </section>
            }
        })
        .collect_view();
    view! { <div class="parts" data-testid="parts">{chips}</div>{sections} }.into_any()
}

/// The network view: every device's interfaces and what the mDNS meter
/// hears on the Dante leg, as parts, the ones in trouble first.
#[component]
fn NetworkView(d: Devices) -> impl IntoView {
    let md = expect_context::<MdnsNow>();
    let (_, slow) = net::summary(&net::rows(&d));
    // The parts' states: the page is laid out again only when one changes.
    let states = Memo::new(move |_| {
        md.0.with(|m| match m {
            Some(m) => [
                if mdns::summary(m).1 { "bad" } else { "ok" },
                if mdns::browser_summary(m).1 {
                    "warn"
                } else {
                    "ok"
                },
                if mdns::floor_summary(m).1 {
                    "bad"
                } else {
                    "ok"
                },
            ],
            None => ["none"; 3],
        })
    });
    move || {
        let [senders, browsers, floor] = states.get();
        parts_view(vec![
            Part {
                id: "links",
                title: "Interfaces",
                state: if slow { "warn" } else { "ok" },
                body: links_table(&d),
            },
            Part {
                id: "mdns",
                title: "mDNS senders",
                state: senders,
                body: view! { <MdnsSenders/> }.into_any(),
            },
            Part {
                id: "browsers",
                title: "96 s browsers",
                state: browsers,
                body: view! { <MdnsBrowsers/> }.into_any(),
            },
            Part {
                id: "floor",
                title: "dstack network floor",
                state: floor,
                body: view! { <MdnsFloor/> }.into_any(),
            },
        ])
    }
}

/// Every device's interfaces, the slow or missing links first.
fn links_table(d: &Devices) -> AnyView {
    let rows = net::rows(d);
    let (line, warn) = net::summary(&rows);
    let rows = parts::trouble_first(rows, |r| r.warn);
    let cell =
        |p: &Option<dcontroller_api::NetPort>| p.as_ref().map(net::port_text).unwrap_or_default();
    view! {
        <p class=if warn { "status error" } else { "status" } data-testid="network-summary">{line}</p>
        <div class="clock-wrap"><table class="clock" data-testid="network-table">
            <thead><tr>
                <th>"Device"</th><th>"Primary"</th><th>"MAC"</th><th>"Netmask"</th>
                <th>"Gateway"</th><th>"Secondary"</th>
            </tr></thead>
            <tbody>
                {rows.into_iter().map(|r| {
                    let name = r.device.clone();
                    match &r.primary {
                        Some(p) => view! {
                            <tr class=if r.warn { "warn" } else { "" } data-testid="network-row" data-name=name>
                                <td>{pages::dev_link(&r.device)}</td>
                                <td data-testid="network-primary">{net::port_text(p)}</td>
                                <td>{p.mac.clone()}</td>
                                <td>{p.netmask.clone()}</td>
                                <td>{p.gateway.clone()}</td>
                                <td data-testid="network-secondary">{cell(&r.secondary)}</td>
                            </tr>
                        }.into_any(),
                        None => view! {
                            <tr class="none" data-testid="network-row" data-name=name>
                                <td>{pages::dev_link(&r.device)}</td>
                                <td colspan="5" data-testid="network-none">
                                    {r.error.clone().map_or_else(|| "not read yet".to_owned(), |e| format!("no network info: {e}"))}
                                </td>
                            </tr>
                        }.into_any(),
                    }
                }).collect_view()}
            </tbody>
        </table></div>
    }
    .into_any()
}

/// A sender's row: `warn` over the cap.
fn row_class(per_s: f64, cap: f64) -> &'static str {
    if per_s > cap { "warn" } else { "" }
}

/// The mDNS senders on the Dante leg: the last minute's loudest sources.
#[component]
fn MdnsSenders() -> impl IntoView {
    let md = expect_context::<MdnsNow>();
    move || {
        let Some(m) = md.0.get() else {
            return view! { <p class="status" data-testid="mdns-summary">"Reading the mDNS meter…"</p> }.into_any();
        };
        let (line, warn) = mdns::summary(&m);
        let cap = m.cap_per_s as f64;
        let top = m.minute.map(|x| x.top).unwrap_or_default();
        view! {
            <p class=if warn { "status error" } else { "status" } data-testid="mdns-summary">{line}</p>
            {(!top.is_empty()).then(|| view! {
                <div class="clock-wrap"><table class="clock" data-testid="mdns-table">
                    <thead><tr><th>"Source"</th><th>"Devices at that address"</th><th>"Datagrams/s"</th></tr></thead>
                    <tbody>
                        {top.into_iter().map(|s| view! {
                            <tr class=row_class(s.per_s, cap) data-testid="mdns-row" data-source=s.source.clone()>
                                <td>{s.source.clone()}</td>
                                <td>{pages::dev_links(&s.devices)}</td>
                                <td data-testid="mdns-rate">{format!("{:.1}", s.per_s)}</td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table></div>
            })}
        }.into_any()
    }
}

/// The hosts that refresh Dante records on the 96 s cycle.
#[component]
fn MdnsBrowsers() -> impl IntoView {
    let md = expect_context::<MdnsNow>();
    move || {
        let Some(m) = md.0.get() else {
            return view! { <p class="status" data-testid="browser-summary">"Reading the mDNS meter…"</p> }.into_any();
        };
        let (line, warn) = mdns::browser_summary(&m);
        let browsers = m.minute.map(|x| x.browsers).unwrap_or_default();
        view! {
            <p class=if warn { "status error" } else { "status" } data-testid="browser-summary">{line}</p>
            {(!browsers.is_empty()).then(|| view! {
                <div class="clock-wrap"><table class="clock" data-testid="browser-table">
                    <thead><tr><th>"Host"</th><th>"Devices at that address"</th><th>"Cycle, s"</th></tr></thead>
                    <tbody>
                        {browsers.into_iter().map(|b| view! {
                            <tr class="warn" data-testid="browser-row" data-source=b.source.clone()>
                                <td>{b.source.clone()}</td>
                                <td>{pages::dev_links(&b.devices)}</td>
                                <td data-testid="browser-period">{format!("{:.1}", b.period_s)}</td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table></div>
            })}
        }.into_any()
    }
}

/// The dstack devices and clients below the network floor.
#[component]
fn MdnsFloor() -> impl IntoView {
    let md = expect_context::<MdnsNow>();
    move || {
        let Some(m) = md.0.get() else {
            return view! { <p class="status" data-testid="floor-summary">"Reading the mDNS meter…"</p> }.into_any();
        };
        let (line, warn) = mdns::floor_summary(&m);
        let below = m.below_floor.clone();
        view! {
            <p class=if warn { "status error" } else { "status" } data-testid="floor-summary">{line}</p>
            {(!below.is_empty()).then(|| view! {
                <div class="clock-wrap"><table class="clock" data-testid="floor-table">
                    <thead><tr><th>"Device"</th><th>"Kind"</th><th>"Address"</th><th>"dstack"</th></tr></thead>
                    <tbody>
                        {below.into_iter().map(|b| view! {
                            <tr class="warn" data-testid="floor-row" data-name=b.name.clone()>
                                <td>{b.name.clone()}</td>
                                <td>{b.kind.clone()}</td>
                                <td>{b.address.clone().unwrap_or_else(|| "local".into())}</td>
                                <td data-testid="floor-version">{b.version.clone()}</td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table></div>
            })}
        }.into_any()
    }
}

/// The clock view: Dante's PTP as Dante Controller's Clock Status tab, the
/// devices' dsync clocks and the PCs' dsync services, as parts, the ones
/// in trouble first.
#[component]
fn ClockView(d: Devices) -> impl IntoView {
    let Clk(clk) = expect_context::<Clk>();
    let (_, ptp_bad) = clock::summary(&clock::rows(&d));
    // The parts' states: the page is laid out again only when one changes.
    let states = Memo::new(move |_| {
        clk.with(|c| match c {
            Some(Ok(c)) => (
                clock::dsync_state(c),
                (!c.services.is_empty()).then(|| {
                    if clock::service_summary(c).1 {
                        "bad"
                    } else {
                        "ok"
                    }
                }),
            ),
            Some(Err(_)) => ("bad", None),
            None => ("none", None),
        })
    });
    move || {
        let (dsync, services) = states.get();
        let mut parts = vec![
            Part {
                id: "ptp",
                title: "Dante PTP",
                state: if ptp_bad { "bad" } else { "ok" },
                body: ptp_table(&d),
            },
            Part {
                id: "dsync",
                title: "dsync in the devices",
                state: dsync,
                body: view! { <DsyncClocks d=d.clone()/> }.into_any(),
            },
        ];
        if let Some(state) = services {
            parts.push(Part {
                id: "dsync-services",
                title: "dsync service nodes",
                state,
                body: view! { <DsyncServices/> }.into_any(),
            });
        }
        parts_view(parts)
    }
}

/// Every device's PTP role and sync, the ones not in sync first.
fn ptp_table(d: &Devices) -> AnyView {
    let rows = clock::rows(d);
    let (line, warn) = clock::summary(&rows);
    let rows = parts::trouble_first(rows, |r| r.warn);
    let known: Vec<String> = d.devices.iter().map(|x| x.name.clone()).collect();
    view! {
        <p class=if warn { "status error" } else { "status" } data-testid="clock-summary">{line}</p>
        <div class="clock-wrap"><table class="clock" data-testid="clock-table">
            <thead><tr>
                <th>"Device"</th><th>"Role"</th><th>"Sync"</th><th>"Clock"</th>
                <th>"Frequency offset"</th><th>"Master"</th><th>"Word clock"</th>
            </tr></thead>
            <tbody>
                {rows.into_iter().map(|r| {
                    let class = if r.warn { "warn" } else if r.clock.as_ref().is_some_and(|c| c.leader) { "lead" } else { "" };
                    let name = r.device.clone();
                    match r.clock {
                        Some(c) => view! {
                            <tr class=class data-testid="clock-row" data-name=name>
                                <td>{pages::dev_link(&r.device)}</td>
                                <td data-testid="clock-role">{clock::role(&c)}</td>
                                <td data-testid="clock-sync">{c.sync.clone()}</td>
                                <td>{c.state.clone()}</td>
                                <td>{clock::ppb(c.frequency_ppb)}</td>
                                <td data-testid="clock-master">{r.master.map(|m| {
                                    // A master the list knows is a device; else its clock id.
                                    if known.iter().any(|k| k.eq_ignore_ascii_case(&m)) {
                                        pages::dev_link(&m).into_any()
                                    } else {
                                        view! { {m} }.into_any()
                                    }
                                })}</td>
                                <td>{c.word_clock.clone().unwrap_or_default()}</td>
                            </tr>
                        }.into_any(),
                        None => view! {
                            <tr class="none" data-testid="clock-row" data-name=name>
                                <td>{pages::dev_link(&r.device)}</td>
                                <td colspan="6" data-testid="clock-none">
                                    {r.error.map_or_else(|| "not read yet".to_owned(), |e| format!("no clock status: {e}"))}
                                </td>
                            </tr>
                        }.into_any(),
                    }
                }).collect_view()}
            </tbody>
        </table></div>
    }
    .into_any()
}

/// The PCs' dsync clock services (#124), as each last answered its status
/// over the LAN. The state chip carries the phase error, so the table
/// holds what it does not say.
#[component]
fn DsyncServices() -> impl IntoView {
    let Clk(clk) = expect_context::<Clk>();
    move || {
        let Some(Ok(c)) = clk.get() else {
            return ().into_any();
        };
        let (line, bad) = clock::service_summary(&c);
        let dash = || "–".to_owned();
        let rows = clock::service_order(&c)
            .into_iter()
            .map(|n| {
                let (state, why) = n.health();
                let gm = match (&n.gm_uuid, &n.gm_source_ip) {
                    (Some(id), Some(ip)) => format!("{id} · {ip}"),
                    (Some(id), None) => id.clone(),
                    (None, Some(ip)) => ip.clone(),
                    (None, None) => dash(),
                };
                let heard = match (n.ptp_rx_pps, n.last_ptp_rx_age_s) {
                    (Some(p), Some(a)) => format!("{p:.0}/s · last {a:.1} s ago"),
                    (Some(p), None) => format!("{p:.0}/s"),
                    (None, Some(a)) => format!("last {a:.1} s ago"),
                    (None, None) => dash(),
                };
                view! {
                    <tr class=if state == "bad" { "warn" } else { "" } data-testid="dsync-service" data-name=n.node.clone() data-state=state>
                        <td title=n.node.clone()>{clock::node_label(&n.node).to_owned()}</td>
                        <td><span class=format!("chip {state}") data-testid="dsync-service-state">{why}</span></td>
                        <td>{n.mode.clone().unwrap_or_else(dash)}</td>
                        <td data-testid="dsync-service-ptp">{heard}</td>
                        <td class="mono">{gm}</td>
                        <td class="num">{n.drift_ppm.map_or_else(dash, |d| format!("{d:+.2} ppm"))}</td>
                        <td>{n.version.clone().unwrap_or_else(dash)}</td>
                        <td class="num">{n.age_s.map_or_else(|| "never".to_owned(), |a| format!("{a} s ago"))}</td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <p class=if bad { "status error" } else { "status" } data-testid="dsync-service-summary">{line}</p>
            <div class="clock-wrap"><table class="clock" data-testid="dsync-services">
                <thead><tr>
                    <th>"Node"</th><th>"Clock"</th><th>"Mode"</th><th>"PTP heard"</th>
                    <th>"Grandmaster"</th><th>"Drift"</th><th>"dsync"</th><th>"Answered"</th>
                </tr></thead>
                <tbody>{rows}</tbody>
            </table></div>
        }
        .into_any()
    }
}

/// The devices' dsync clocks (#124): what each dstack device says of its
/// own time, the troubled ones on top; the devices that say nothing are a
/// count, their names one click away.
#[component]
fn DsyncClocks(d: Devices) -> impl IntoView {
    let Clk(clk) = expect_context::<Clk>();
    move || {
        let c = match clk.get() {
            None => return view! { <p class="status">"Reading the dsync clocks…"</p> }.into_any(),
            Some(Err(e)) => {
                return view! { <p class="status error">{format!("dsync clocks not readable: {e}")}</p> }
                    .into_any();
            }
            Some(Ok(c)) => c,
        };
        let (line, bad) = clock::dsync_summary(&c);
        let silent = clock::dsync_silent(&d, &c);
        let dash = || "–".to_owned();
        let rows = clock::dsync_order(&c)
            .into_iter()
            .map(|n| {
                let (state, why) = n.health();
                let source = match n.source.as_str() {
                    "follower" => "its own follower",
                    "service" => "the PC's dsync service",
                    "system" => "test system clock",
                    _ => "none",
                };
                let f = n.follower.as_ref();
                let s = n.service.as_ref();
                let gm = f
                    .and_then(|f| f.grandmaster.clone())
                    .or_else(|| s.and_then(|s| s.grandmaster.clone()))
                    .unwrap_or_else(dash);
                view! {
                    <tr class=if state == "bad" { "warn" } else { "" } data-testid="dsync-row" data-name=n.device.clone() data-state=state>
                        <td>{pages::dev_link(&n.device)}</td>
                        <td><span class=format!("chip {state}") data-testid="dsync-state">{why}</span></td>
                        <td>{source}</td>
                        <td class="num">{f.and_then(|f| f.error_p90_ns).map_or_else(dash, clock::ns_text)}</td>
                        <td>{f.map_or_else(dash, |f| f.waiting.clone().unwrap_or_else(|| "in use".to_owned()))}</td>
                        <td class="num">{f.map_or_else(dash, |f| f.holdovers.to_string())}</td>
                        <td>{s.map_or_else(dash, |s| match s.reply_age_ms {
                            Some(ms) => format!("{} · reply {} ms ago", s.poll, ms),
                            None => s.poll.clone(),
                        })}</td>
                        <td class="mono">{gm}</td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <p class=if bad { "status error" } else { "status" } data-testid="dsync-summary">{line}</p>
            {(!c.nodes.is_empty()).then(|| view! {
                <div class="clock-wrap"><table class="clock" data-testid="dsync-table">
                    <thead><tr>
                        <th>"Device"</th><th>"Clock"</th><th>"Runs on"</th><th>"Follower error (p90)"</th>
                        <th>"Follower"</th><th>"Holdovers"</th><th>"dsync service"</th><th>"Grandmaster"</th>
                    </tr></thead>
                    <tbody>{rows}</tbody>
                </table></div>
            })}
            {(!silent.is_empty()).then(|| view! {
                <details class="muted small silent" data-testid="dsync-silent">
                    <summary>{format!(
                        "{} device{} no dsync figures (not a dstack device, or a dstack before v0.6.187)",
                        silent.len(),
                        if silent.len() == 1 { " gives" } else { "s give" },
                    )}</summary>
                    <p>{silent.join(", ")}</p>
                </details>
            })}
        }
        .into_any()
    }
}

/// Everything a device's card holds under its summary line: its settings,
/// clock, network and product rows, the rename and change buttons, the
/// last read's error, its RX channels with their sources, its TX channels
/// and flows. One piece, so the card and the map's device detail (#122)
/// show the same.
#[component]
fn DeviceBody(dev: Device) -> impl IntoView {
    let flows_name = dev.name.clone();
    view! {
        {Some({
            let mut rows = settings_rows(&dev.settings.clone().unwrap_or_default(), dev.info.as_ref());
            rows.extend(dev.clock.as_ref().map(clock_rows).unwrap_or_default());
            rows.extend(dev.network.as_deref().map(network_rows).unwrap_or_default());
            rows
        }).filter(|r| !r.is_empty()).map(|rows| view! {
            <table class="settings" data-testid="settings-table">
                {rows.into_iter().map(|(k, v)| view! { <tr><th>{k}</th><td>{v}</td></tr> }).collect_view()}
            </table>
        })}
        <div class="actions">
            <button data-testid="rename-device" on:click={
                let name = dev.name.clone();
                move |_| {
                    let ctl = expect_context::<Ctl>();
                    if let Some(Ok(d)) = ctl.devices.get_untracked() {
                        ctl.renaming.set(Some(RenameAsk::device(&d, &name)));
                    }
                }
            }>"Rename device"</button>
            {settings::asks(&dev).into_iter().map(|a| {
                let id = format!("change-{}", a.kind.name().replace(' ', "-"));
                let label = format!("Change {}", a.kind.name());
                view! {
                    <button data-testid=id on:click=move |_| {
                        expect_context::<Ctl>().setting.set(Some(a.clone()));
                    }>{label}</button>
                }
            }).collect_view()}
        </div>
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
                {c.tx.clone().into_iter().map(|t| view! {
                    <li data-testid="tx-channel" value=t.channel>{t.label}</li>
                }).collect_view()}
            </ol>
            <TxFlowsBox device=flows_name.clone() tx=c.tx.iter().map(|t| (t.channel, t.label.clone())).collect()/>
        })}
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

    #[test]
    fn an_mdns_alarm_entry_names_the_sender_and_its_devices() {
        let e = ChangeEntry {
            kind: "status".into(),
            what: "mdns".into(),
            rx_device: "mixer, this server".into(),
            rx_label: "10.0.0.7".into(),
            ..ChangeEntry::default()
        };
        assert_eq!(
            change_target(&e),
            "mDNS sender 10.0.0.7 (mixer, this server)"
        );
    }

    #[test]
    fn the_browser_and_floor_alarm_entries_name_their_host() {
        let mut e = ChangeEntry {
            kind: "status".into(),
            what: "mdns-browser".into(),
            rx_device: "PC-Studio".into(),
            rx_label: "10.0.0.7".into(),
            ..ChangeEntry::default()
        };
        assert_eq!(change_target(&e), "96 s browser 10.0.0.7 (PC-Studio)");
        e.what = "network-floor".into();
        assert_eq!(change_target(&e), "dstack PC-Studio at 10.0.0.7");
    }

    #[test]
    fn a_status_entry_reads_as_receiver_and_source() {
        let e = ChangeEntry {
            kind: "status".into(),
            rx_device: "band".into(),
            rx_label: "Hand1 Mic".into(),
            after: Some(Source {
                channel: "HAND 1 01".into(),
                device: "wireless-mics-01".into(),
            }),
            ..ChangeEntry::default()
        };
        assert_eq!(
            change_target(&e),
            "Hand1 Mic @ band \u{2190} HAND 1 01 @ wireless-mics-01"
        );
    }

    #[test]
    fn only_another_server_version_asks_for_a_reload() {
        assert!(new_version("v0.1.36 (e04ca65)", "v0.1.37 (1234567)\n"));
        assert!(!new_version("v0.1.36 (e04ca65)", "v0.1.36 (e04ca65)\n"));
        // A login page or an empty answer is no new version.
        assert!(!new_version("v0.1.36 (e04ca65)", ""));
        assert!(!new_version("v0.1.36 (e04ca65)", "<!doctype html><html>"));
    }

    fn devices(enabled: bool, error: Option<&str>, n: usize) -> Devices {
        Devices {
            started_at: 0,
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

    #[test]
    fn settings_line_and_rows_show_what_the_device_reports() {
        let s = Settings {
            sample_rate: Some(48000),
            pullup: Some(0),
            latency_configured_us: Some(1000),
            latency_active_us: Some(1000),
            latency_default_us: Some(1000),
            latency_min_us: Some(150),
            latency_max_us: Some(5000),
        };
        assert_eq!(
            settings_line(&s, None).as_deref(),
            Some("48 kHz · latency 1.00 ms")
        );
        let rows = settings_rows(&s, None);
        assert_eq!(rows[0], ("Sample rate", "48000 Hz".to_owned()));
        assert_eq!(rows[1], ("Pull-up", "none".to_owned()));
        assert_eq!(
            rows.last().unwrap(),
            &("Latency, range", "0.15 ms – 5.00 ms".to_owned())
        );
        // A device that reports nothing shows nothing.
        assert_eq!(settings_line(&Settings::default(), None), None);
        assert!(settings_rows(&Settings::default(), None).is_empty());
        let r441 = Settings {
            sample_rate: Some(44100),
            ..Settings::default()
        };
        assert_eq!(settings_line(&r441, None).as_deref(), Some("44.1 kHz"));
    }

    #[test]
    fn device_info_adds_encoding_and_versions() {
        let s = Settings {
            sample_rate: Some(96000),
            latency_active_us: Some(250),
            ..Settings::default()
        };
        let i = Info {
            manufacturer: Some("Tascam".into()),
            model: Some("ML-32D".into()),
            firmware_version: Some("4.2.5.1".into()),
            platform: Some("Brooklyn II".into()),
            encoding_bits: Some(24),
            ..Info::default()
        };
        assert_eq!(
            settings_line(&s, Some(&i)).as_deref(),
            Some("96 kHz · 24 bit · latency 0.25 ms")
        );
        let rows = settings_rows(&s, Some(&i));
        assert_eq!(rows[1], ("Encoding", "PCM 24 bit".to_owned()));
        assert!(rows.contains(&("Firmware", "4.2.5.1".to_owned())));
        assert!(rows.contains(&("Dante platform", "Brooklyn II".to_owned())));
        assert!(!rows.iter().any(|(k, _)| *k == "Software"));
        // Info alone, without settings, still shows.
        assert_eq!(
            settings_line(&Settings::default(), Some(&i)).as_deref(),
            Some("24 bit")
        );
        assert_eq!(settings_rows(&Settings::default(), Some(&i)).len(), 5);
    }
}
