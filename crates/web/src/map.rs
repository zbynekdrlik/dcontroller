//! The map (dcontroller #122): what is connected to what, and how each
//! connection did in the last hour. The connections come from the devices'
//! RX subscriptions (the network as it is now), their figures from the
//! server's link summaries; a bridge device's two delays are two more
//! connections, to its ASIO side. Pure, so it is tested on the host: the
//! graph, its layout in three columns, the SVG and the hash of a choice.

use dcontroller_api::{Devices, LinkSummary, Links};

/// How a connection did in the last hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// Nothing recorded for it.
    Unknown,
    Ok,
    /// Late packets earlier in the day, or a peak at 90 % of the latency.
    Warn,
    /// Late packets (underruns for a bridge) in the last hour.
    Bad,
}

impl State {
    pub fn class(self) -> &'static str {
        match self {
            State::Unknown => "unknown",
            State::Ok => "ok",
            State::Warn => "warn",
            State::Bad => "bad",
        }
    }
}

/// One connection, as the hash names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sel {
    /// A source device into a receiver.
    Link { from: String, to: String },
    /// A bridge device's own path: its ASIO side to Dante, or back.
    Bridge { device: String, to_dante: bool },
    /// A device: its detail shows, with its connections.
    Device { name: String },
    /// A video connection camera-box's nodes report (#124 phase 1), by id.
    Video { id: String },
}

/// Where a link to a page the Map took over lands (#130): the Overview on
/// the Map, the Devices list on its Devices table, one device (or an old
/// latency link) on that device's detail. `None`: not an old page.
pub fn moved(hash: &str) -> Option<String> {
    let path = hash.strip_prefix('#')?;
    let (head, rest) = path.split_once('/').unwrap_or((path, ""));
    match head {
        "overview" => Some("#map".to_owned()),
        "devices" | "latency" => Some(
            decode(rest)
                .filter(|n| !n.is_empty() && !n.contains('/'))
                .map_or_else(
                    || "#map/devices".to_owned(),
                    |name| Sel::Device { name }.hash(),
                ),
        ),
        _ => None,
    }
}

/// The table of the Map a hash opens on (`#map/links`, `#map/devices`,
/// `#map/schema`); `None` keeps the one shown.
pub fn tab_of(hash: &str) -> Option<&'static str> {
    match hash {
        "#map/schema" => Some("schema"),
        "#map/links" => Some("links"),
        "#map/devices" => Some("devices"),
        _ => None,
    }
}

/// `text` with every byte outside `A-Z a-z 0-9 - _ . ~` as `%XX`.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// [`encode`] undone; `None` for a broken escape or bytes that are no UTF-8.
pub fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

impl Sel {
    /// The page's hash for this connection.
    pub fn hash(&self) -> String {
        match self {
            Sel::Link { from, to } => format!("#map/link/{}/{}", encode(from), encode(to)),
            Sel::Bridge { device, to_dante } => format!(
                "#map/bridge/{}/{}",
                encode(device),
                if *to_dante { "to-dante" } else { "to-asio" }
            ),
            Sel::Device { name } => format!("#map/device/{}", encode(name)),
            Sel::Video { id } => format!("#map/video/{}", encode(id)),
        }
    }

    /// The connection a hash names, if it names one.
    pub fn from_hash(hash: &str) -> Option<Sel> {
        let mut parts = hash.strip_prefix("#map/")?.split('/');
        let (kind, a) = (parts.next()?, parts.next()?);
        if kind == "device" || kind == "video" {
            return match (parts.next(), kind) {
                (None, "device") => Some(Sel::Device { name: decode(a)? }),
                (None, _) => Some(Sel::Video { id: decode(a)? }),
                (Some(_), _) => None,
            };
        }
        let b = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        match (kind, b) {
            ("link", _) => Some(Sel::Link {
                from: decode(a)?,
                to: decode(b)?,
            }),
            ("bridge", "to-dante" | "to-asio") => Some(Sel::Bridge {
                device: decode(a)?,
                to_dante: b == "to-dante",
            }),
            _ => None,
        }
    }

    /// The device whose history holds this connection: the receiver, or
    /// the bridge; a device's own (its chart of every connection in, or
    /// its bridge's both ways, as the Devices page had them, #130).
    pub fn device(&self) -> Option<&str> {
        match self {
            Sel::Link { to, .. } => Some(to),
            Sel::Bridge { device, .. } => Some(device),
            Sel::Device { name } => Some(name),
            // Its own history (`/api/video/<id>`), not a device's.
            Sel::Video { .. } => None,
        }
    }

    /// The same connection, the names' case aside.
    pub fn same(&self, other: &Sel) -> bool {
        match (self, other) {
            (Sel::Link { from: a, to: b }, Sel::Link { from: c, to: d }) => {
                a.eq_ignore_ascii_case(c) && b.eq_ignore_ascii_case(d)
            }
            (
                Sel::Bridge {
                    device: a,
                    to_dante: x,
                },
                Sel::Bridge {
                    device: b,
                    to_dante: y,
                },
            ) => a.eq_ignore_ascii_case(b) && x == y,
            (Sel::Device { name: a }, Sel::Device { name: b }) => a.eq_ignore_ascii_case(b),
            (Sel::Video { id: a }, Sel::Video { id: b }) => a == b,
            _ => false,
        }
    }
}

/// What a box stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Device,
    /// A source some receiver is subscribed to that is not on the network.
    Missing,
    /// A bridge's ASIO side: no Dante device.
    Asio,
    /// A video node camera-box reports (#124) that is no Dante device.
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub kind: NodeKind,
    /// Two small lines under the name.
    pub lines: [String; 2],
    /// The clock's word and whether it is good.
    pub badge: Option<(String, bool)>,
    /// Its own dsync clock (the device's follower or its PC's service,
    /// #124): the word and whether it is good; `None` when it gives none.
    pub dsync: Option<(String, bool)>,
    /// For an ASIO side: its bridge's node.
    pub of: Option<usize>,
    /// TX and RX channel counts (the column of a device with no connection).
    pub tx: usize,
    pub rx: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub sel: Sel,
    pub from: usize,
    pub to: usize,
    pub state: State,
    /// On the line: the receiver's latency (a bridge: its hold time).
    pub label: String,
    /// On the line when audio was lost in the last hour: `12 late`.
    pub lost: Option<String>,
    /// The hover text.
    pub title: String,
    /// How bad, for the worst-first order: lost in the hour, then the
    /// peak's share of the latency (per mille).
    pub weight: (u64, u32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Where the owner put each device (#122): (device, place), the places
    /// in the order their areas stand. Empty: the map is laid out by who
    /// sends and who receives.
    pub places: Vec<(String, String)>,
}

pub fn ms(us: u32) -> String {
    format!("{:.2} ms", f64::from(us) / 1000.0)
}

/// A latency as a line of the map carries it: no trailing zeros
/// (`5 ms`, `0.25 ms`, `8.36 ms`), so the label stays short (#122).
pub fn ms_short(us: u32) -> String {
    trimmed(f64::from(us) / 1000.0)
}

fn trimmed(ms: f64) -> String {
    let s = format!("{ms:.2}");
    format!("{} ms", s.trim_end_matches('0').trim_end_matches('.'))
}

/// What the owner switched on over the schema (#122).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Show {
    /// Only the connections in trouble stay bright.
    pub problems: bool,
    /// A device looked for by name, lower case; empty for none.
    pub find: String,
}

/// `n` with a space between thousands.
pub fn num(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The window a connection's colour speaks of (#142): the Map shows the
/// last hour unless the 3 h or 24 h switch is on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Span {
    #[default]
    H1,
    H3,
    H24,
}

impl Span {
    pub const ALL: [Span; 3] = [Span::H1, Span::H3, Span::H24];

    pub fn label(self) -> &'static str {
        match self {
            Span::H1 => "1 h",
            Span::H3 => "3 h",
            Span::H24 => "24 h",
        }
    }

    /// In words, after "in the last …".
    pub fn words(self) -> &'static str {
        match self {
            Span::H1 => "hour",
            Span::H3 => "3 hours",
            Span::H24 => "24 hours",
        }
    }

    /// Late packets in the window.
    pub fn late(self, l: &LinkSummary) -> u64 {
        match self {
            Span::H1 => l.late_1h,
            Span::H3 => l.late_3h.max(l.late_1h),
            Span::H24 => l.late_24h,
        }
    }

    /// The window's peak closest to its latency, and that latency (a
    /// server without the longer windows: the hour's).
    pub fn peak(self, l: &LinkSummary) -> (u32, u32) {
        let (p, lat) = match self {
            Span::H1 => (l.peak_us, l.latency_us),
            Span::H3 => (l.peak_3h_us, l.latency_3h_us),
            Span::H24 => (l.peak_24h_us, l.latency_24h_us),
        };
        if lat == 0 {
            (l.peak_us, l.latency_us)
        } else {
            (p, lat)
        }
    }
}

/// A connection's state in a window, as the mockup named it (#142): red,
/// late packets in it; amber, a peak at 90 % of the latency or more in
/// it; green otherwise. Late packets earlier than the window are no
/// colour: the 24 h switch shows them.
pub fn link_state(l: &LinkSummary, span: Span) -> State {
    let (peak, lat) = span.peak(l);
    if span.late(l) > 0 {
        State::Bad
    } else if lat > 0 && u64::from(peak) * 10 >= u64::from(lat) * 9 {
        State::Warn
    } else {
        State::Ok
    }
}

fn per_mille(l: &LinkSummary) -> u32 {
    if l.latency_us == 0 {
        return 0;
    }
    u32::try_from(u64::from(l.peak_us) * 1000 / u64::from(l.latency_us)).unwrap_or(u32::MAX)
}

/// Each device's own dsync clock as its box's second badge (#124), from
/// what the devices say of their clocks (`/api/clocks`).
pub fn with_dsync(g: &mut Graph, c: &dcontroller_api::Clocks) {
    for n in g.nodes.iter_mut().filter(|n| n.kind == NodeKind::Device) {
        n.dsync = c
            .nodes
            .iter()
            .find(|x| x.device.eq_ignore_ascii_case(&n.name))
            .map(|x| match x.health().0 {
                "ok" => ("dsync locked".to_owned(), true),
                "warn" => ("dsync limping".to_owned(), false),
                _ => ("dsync no time".to_owned(), false),
            });
    }
}

/// A video connection's state in a window, as a Dante line's (#124): red
/// with frames lost in it, amber with frames duplicated or skipped, green
/// otherwise; not measured while its node gave no latency.
pub fn video_state(c: &dcontroller_api::VideoConn, span: Span) -> State {
    let (lost, dup, skip) = video_window(c, span);
    if lost > 0 {
        State::Bad
    } else if dup + skip > 0 {
        State::Warn
    } else if c.latency_ms.is_none() {
        State::Unknown
    } else {
        State::Ok
    }
}

/// A video connection's frames lost, duplicated and skipped in a window.
pub fn video_window(c: &dcontroller_api::VideoConn, span: Span) -> (u64, u64, u64) {
    match span {
        Span::H1 => (c.lost_1h, c.duplicated_1h, c.skipped_1h),
        Span::H3 => (c.lost_3h, c.duplicated_3h, c.skipped_3h),
        Span::H24 => (c.lost_24h, c.duplicated_24h, c.skipped_24h),
    }
}

/// The video network into the graph (#124 phase 1): a box per video node
/// that is no Dante device already (one name, one box), a line per video
/// connection, coloured by the window as a Dante line.
pub fn with_video(g: &mut Graph, v: &dcontroller_api::VideoReport, span: Span) {
    let node = |g: &mut Graph, name: &str| -> usize {
        if let Some(i) = g
            .nodes
            .iter()
            .position(|n| n.kind != NodeKind::Asio && n.name.eq_ignore_ascii_case(name))
        {
            return i;
        }
        let what = v
            .nodes
            .iter()
            .find(|n| n.name.eq_ignore_ascii_case(name))
            .map_or_else(|| "video".to_owned(), |n| format!("video · {}", n.kind));
        g.nodes.push(Node {
            name: name.to_owned(),
            kind: NodeKind::Video,
            lines: [what, String::new()],
            badge: None,
            dsync: None,
            of: None,
            tx: 0,
            rx: 0,
        });
        g.nodes.len() - 1
    };
    for c in &v.connections {
        let from = node(g, &c.source);
        let to = node(g, &c.sink);
        let state = video_state(c, span);
        let (lost, dup, skip) = video_window(c, span);
        let label = c.latency_ms.map_or_else(|| "–".to_owned(), trimmed);
        g.edges.push(Edge {
            sel: Sel::Video { id: c.id.clone() },
            from,
            to,
            state,
            label: label.clone(),
            lost: (lost > 0).then(|| format!("{lost} lost")),
            title: format!(
                "{} → {} · {} {} · latency {label} · frames lost {lost}, duplicated {dup}, skipped {skip} in the last {}",
                c.source,
                c.sink,
                c.kind,
                c.sink_input,
                span.words()
            ),
            weight: (lost, u32::try_from(dup + skip).unwrap_or(u32::MAX)),
        });
    }
}

/// The summary of the connection `from` into `to`, the names' case aside.
pub fn summary_of<'a>(links: Option<&'a Links>, from: &str, to: &str) -> Option<&'a LinkSummary> {
    links?
        .links
        .iter()
        .find(|l| l.from.eq_ignore_ascii_case(from) && l.to.eq_ignore_ascii_case(to))
}

/// The graph of `d`'s devices and their connections, with `links`'
/// figures: a box per device (and per subscribed source that is not on the
/// network, and per bridge's ASIO side), a line per (source, receiver) pair
/// with a subscription and two per bridge.
pub fn build(d: &Devices, links: Option<&Links>, span: Span) -> Graph {
    let mut g = Graph::default();
    for dev in &d.devices {
        let (tx, rx) = dev
            .channels
            .as_ref()
            .map_or((0, 0), |c| (c.tx.len(), c.rx.len()));
        let what = [dev.manufacturer.clone(), dev.model.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        let badge = dev.clock.as_ref().map(|c| {
            if c.grandmaster {
                ("PTP GM".to_owned(), true)
            } else if c.sync == "synchronized" {
                ("clock ok".to_owned(), true)
            } else {
                (format!("clock {}", c.sync), false)
            }
        });
        g.nodes.push(Node {
            name: dev.name.clone(),
            kind: NodeKind::Device,
            lines: [what, format!("{tx} TX · {rx} RX")],
            badge,
            dsync: None,
            of: None,
            tx,
            rx,
        });
    }
    let find = |g: &Graph, name: &str| {
        g.nodes
            .iter()
            .position(|n| n.kind != NodeKind::Asio && n.name.eq_ignore_ascii_case(name))
    };
    for (to, dev) in d.devices.iter().enumerate() {
        let Some(ch) = dev.channels.as_ref() else {
            continue;
        };
        // The source devices of this receiver, in channel order, each with
        // its channel count.
        let mut sources: Vec<(String, usize)> = Vec::new();
        for r in &ch.rx {
            let Some(s) = r.source.as_ref() else {
                continue;
            };
            match sources
                .iter_mut()
                .find(|(n, _)| n.eq_ignore_ascii_case(&s.device))
            {
                Some((_, n)) => *n += 1,
                None => sources.push((s.device.clone(), 1)),
            }
        }
        for (source, channels) in sources {
            let from = match find(&g, &source) {
                Some(i) => i,
                None => {
                    g.nodes.push(Node {
                        name: source.clone(),
                        kind: NodeKind::Missing,
                        lines: ["not on the network".to_owned(), String::new()],
                        badge: None,
                        dsync: None,
                        of: None,
                        tx: 1,
                        rx: 0,
                    });
                    g.nodes.len() - 1
                }
            };
            let sum = summary_of(links, &source, &dev.name);
            let set = dev
                .settings
                .as_ref()
                .and_then(|s| s.latency_active_us.or(s.latency_configured_us));
            let label = sum
                .map(|s| s.latency_us)
                .filter(|&l| l > 0)
                .or(set)
                .map_or_else(|| "–".to_owned(), ms_short);
            let chans = if channels == 1 {
                "1 channel".to_owned()
            } else {
                format!("{channels} channels")
            };
            let title = match sum {
                Some(s) => format!(
                    "{} → {} · {chans} · latency {} · worst peak {} · late packets {} in 1 h, {} in 24 h",
                    g.nodes[from].name,
                    dev.name,
                    ms(s.latency_us),
                    ms(s.peak_us),
                    num(s.late_1h),
                    num(s.late_24h)
                ),
                None => format!(
                    "{} → {} · {chans} · not measured: the receiver reports no packet times",
                    g.nodes[from].name, dev.name
                ),
            };
            g.edges.push(Edge {
                sel: Sel::Link {
                    from: g.nodes[from].name.clone(),
                    to: dev.name.clone(),
                },
                from,
                to,
                state: sum.map_or(State::Unknown, |s| link_state(s, span)),
                label,
                lost: sum
                    .map(|s| span.late(s))
                    .filter(|&n| n > 0)
                    .map(|n| format!("{} late", num(n))),
                title,
                weight: sum.map_or((0, 0), |s| (s.late_1h, per_mille(s))),
            });
        }
    }
    for b in links.map(|l| l.bridges.as_slice()).unwrap_or_default() {
        let Some(dev) = find(&g, &b.device) else {
            continue;
        };
        let name = b.last.asio_name().to_owned();
        g.nodes.push(Node {
            name: name.clone(),
            kind: NodeKind::Asio,
            lines: [
                "the bridge's ASIO side".to_owned(),
                "not a Dante device".to_owned(),
            ],
            badge: None,
            dsync: None,
            of: Some(dev),
            tx: 0,
            rx: 0,
        });
        let asio = g.nodes.len() - 1;
        let (to_dante, to_asio) = b
            .last
            .card_us()
            .unwrap_or((b.last.asio_to_dante_us(), b.last.dante_to_asio_us()));
        // To Dante: what the bridge plays out of ASIO (its from-ASIO side).
        for (dante, us, under) in [
            (true, to_dante, b.from_asio_underruns_1h),
            (false, to_asio, b.to_asio_underruns_1h),
        ] {
            let (a, z) = if dante {
                (name.as_str(), "Dante")
            } else {
                ("Dante", name.as_str())
            };
            g.edges.push(Edge {
                sel: Sel::Bridge {
                    device: g.nodes[dev].name.clone(),
                    to_dante: dante,
                },
                from: if dante { asio } else { dev },
                to: if dante { dev } else { asio },
                state: if under > 0 { State::Bad } else { State::Ok },
                label: ms_short(us),
                lost: (under > 0).then(|| {
                    format!(
                        "{} underrun{}",
                        num(under),
                        if under == 1 { "" } else { "s" }
                    )
                }),
                title: format!(
                    "{a} → {z} inside {} · held {} · underruns {} in 1 h",
                    g.nodes[dev].name,
                    ms(us),
                    num(under)
                ),
                weight: (under, 0),
            });
        }
    }
    g
}

/// The connection to show when none is chosen: the worst (most lost, then
/// the peak closest to its latency), the first of equals.
pub fn worst(g: &Graph) -> Option<&Edge> {
    g.edges
        .iter()
        .fold(None, |best: Option<&Edge>, e| match best {
            Some(b) if (b.state, b.weight) >= (e.state, e.weight) => Some(b),
            _ => Some(e),
        })
}

/// Every connection, the worst first (most lost, then the peak closest to
/// its latency), equals in the graph's order.
pub fn worst_first(g: &Graph) -> Vec<&Edge> {
    let mut all: Vec<&Edge> = g.edges.iter().collect();
    all.sort_by_key(|e| std::cmp::Reverse((e.state, e.weight)));
    all
}

/// The connections' last hour in one line (#122 step 3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    /// Dante connections, and how many of them are measured.
    pub links: usize,
    pub measured: usize,
    /// Measured ones that lost nothing in the last hour.
    pub clean: usize,
    /// Those that lost audio in the last hour, worst first: name and count.
    pub lost: Vec<(String, u64)>,
    /// Video connections that lost frames in the window (#124), worst
    /// first: `source → sink` and the frames.
    pub lost_video: Vec<(String, u64)>,
    /// Those with a peak close to their latency.
    pub warn: usize,
    /// Bridges, and their underruns in the last hour.
    pub bridges: usize,
    pub underruns: u64,
}

/// Every bridge's underruns in the last 24 hours, both ways (#142).
pub fn underruns_24h(l: &Links) -> u64 {
    l.bridges
        .iter()
        .map(|b| b.to_asio_underruns_24h + b.from_asio_underruns_24h)
        .sum()
}

pub fn health(g: &Graph) -> Health {
    let mut h = Health::default();
    for e in worst_first(g) {
        match &e.sel {
            Sel::Link { from, to } => {
                h.links += 1;
                match e.state {
                    State::Unknown => continue,
                    State::Bad => h.lost.push((format!("{from} → {to}"), e.weight.0)),
                    State::Warn => h.warn += 1,
                    State::Ok => {}
                }
                h.measured += 1;
                if e.state != State::Bad {
                    h.clean += 1;
                }
            }
            Sel::Bridge { to_dante, .. } => {
                if *to_dante {
                    h.bridges += 1;
                }
                h.underruns += e.weight.0;
            }
            // A video connection is one more of the rig's (#124): it is
            // clean without frames lost in the window.
            Sel::Video { .. } => {
                h.links += 1;
                match e.state {
                    State::Unknown => continue,
                    State::Bad => h.lost_video.push((
                        format!("{} → {}", g.nodes[e.from].name, g.nodes[e.to].name),
                        e.weight.0,
                    )),
                    State::Warn => h.warn += 1,
                    State::Ok => {}
                }
                h.measured += 1;
                if e.state != State::Bad {
                    h.clean += 1;
                }
            }
            Sel::Device { .. } => {}
        }
    }
    h
}

/// The edge `sel` names, if the graph has it.
pub fn edge_of<'a>(g: &'a Graph, sel: &Sel) -> Option<&'a Edge> {
    g.edges.iter().find(|e| e.sel.same(sel))
}

/// The device's box `name` names (no ASIO side, no missing source).
pub fn node_of(g: &Graph, name: &str) -> Option<usize> {
    g.nodes
        .iter()
        .position(|n| n.kind == NodeKind::Device && n.name.eq_ignore_ascii_case(name))
}

/// Whether the network has what `sel` names.
pub fn has(g: &Graph, sel: &Sel) -> bool {
    match sel {
        Sel::Device { name } => node_of(g, name).is_some(),
        _ => edge_of(g, sel).is_some(),
    }
}

/// A device's connections: into it, out of it, and inside it (a bridge's
/// two ways), each in the graph's order.
pub fn edges_of(g: &Graph, node: usize) -> [Vec<&Edge>; 3] {
    let dante = |e: &&Edge| matches!(e.sel, Sel::Link { .. } | Sel::Video { .. });
    [
        g.edges
            .iter()
            .filter(dante)
            .filter(|e| e.to == node)
            .collect(),
        g.edges
            .iter()
            .filter(dante)
            .filter(|e| e.from == node)
            .collect(),
        g.edges
            .iter()
            .filter(|e| matches!(e.sel, Sel::Bridge { .. }) && (e.from == node || e.to == node))
            .collect(),
    ]
}

// ---- layout ----

pub const WIDTH: f64 = 1300.0;
/// Four columns: sends; two for the devices that send and receive, split
/// so that most of their connections run between the two instead of
/// looping inside one (#142); receives.
const COL_X: [f64; 4] = [30.0, 385.0, 740.0, 1075.0];
const BOX_W: f64 = 195.0;
const BOX_H: f64 = 58.0;
const GAP: f64 = 26.0;
/// Room between a bridge and its ASIO side, for the two lines' labels.
const ASIO_GAP: f64 = 72.0;
const TOP: f64 = 46.0;
const LABEL_H: f64 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn mid_y(&self) -> f64 {
        self.y + self.h / 2.0
    }

    fn hits(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// One line's path and its label's box.
#[derive(Debug, Clone, PartialEq)]
pub struct Geo {
    pub d: String,
    pub label: Rect,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Placed {
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<Rect>,
    /// Each node's column: 0 sends, 1 and 2 send and receive, 3 receives;
    /// with places set, its place's column.
    pub cols: Vec<usize>,
    pub edges: Vec<Geo>,
    /// The words over the columns: where each starts, and what it says.
    pub titles: Vec<(f64, String)>,
    /// With places set: each place's frame.
    pub areas: Vec<Rect>,
}

/// What stands over the devices no place was given to.
pub const NO_PLACE: &str = "no place set";
/// From one place's column to the next: room for the lines, their labels
/// and the arcs between two devices of one place.
const PLACE_PITCH: f64 = BOX_W + 190.0;
const PLACE_EDGE: f64 = 120.0;

/// With places set: each node's column, where the columns start, and their
/// names. A place stands where it was first used; a device with no place
/// goes to a last column of its own; an ASIO side stays under its bridge.
fn by_place(g: &Graph) -> (Vec<usize>, Vec<f64>, Vec<String>) {
    let of = |n: &Node| {
        g.places
            .iter()
            .find(|(d, _)| *d == n.name)
            .map(|(_, p)| p.as_str())
    };
    let mut names: Vec<String> = Vec::new();
    for (_, p) in &g.places {
        if !names.contains(p) && g.nodes.iter().any(|n| of(n) == Some(p.as_str())) {
            names.push(p.clone());
        }
    }
    let alone = |n: &Node| n.kind != NodeKind::Asio && of(n).is_none();
    if g.nodes.iter().any(alone) {
        names.push(NO_PLACE.to_owned());
    }
    let last = names.len().saturating_sub(1);
    let col = |n: &Node| {
        of(n)
            .and_then(|p| names.iter().position(|x| x == p))
            .unwrap_or(last)
    };
    let cols = g
        .nodes
        .iter()
        .map(|n| match n.of {
            Some(bridge) if n.kind == NodeKind::Asio => col(&g.nodes[bridge]),
            _ => col(n),
        })
        .collect();
    let xs = (0..names.len())
        .map(|c| PLACE_EDGE + c as f64 * PLACE_PITCH)
        .collect();
    (cols, xs, names)
}

/// Each node's column by its part: only sending (0), only receiving (3),
/// both (1 or 2). A device with no connection goes by what its channels
/// allow. The devices that do both are split into two columns so that as
/// many of the connections among them as can run from one column to the
/// other: a line inside one column is an arc over its boxes, which made a
/// real network (a dozen such devices) unreadable.
fn columns(g: &Graph) -> Vec<usize> {
    let dante = |e: &&Edge| matches!(e.sel, Sel::Link { .. } | Sel::Video { .. }) && e.from != e.to;
    let outs = |i: usize| g.edges.iter().filter(dante).filter(|e| e.from == i).count();
    let ins = |i: usize| g.edges.iter().filter(dante).filter(|e| e.to == i).count();
    let mut cols: Vec<usize> = g
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| match (outs(i) > 0, ins(i) > 0) {
            (true, false) => 0,
            (false, true) => 3,
            // The more it sends than it takes, the further left.
            (true, true) => {
                if outs(i) >= ins(i) {
                    1
                } else {
                    2
                }
            }
            (false, false) => match (n.tx > 0, n.rx > 0) {
                (true, false) => 0,
                (false, true) => 3,
                _ => 1,
            },
        })
        .collect();
    // Moves a middle device to the other middle column while that leaves
    // fewer connections inside a column; each move strictly lowers that
    // count, so this ends.
    let inside = |cols: &[usize], i: usize| -> isize {
        g.edges
            .iter()
            .filter(dante)
            .filter_map(|e| match (e.from == i, e.to == i) {
                (true, false) => Some(e.to),
                (false, true) => Some(e.from),
                _ => None,
            })
            .filter(|&o| matches!(cols[o], 1 | 2))
            .map(|o| if cols[o] == cols[i] { 1 } else { -1 })
            .sum()
    };
    loop {
        let mut moved = false;
        for i in 0..g.nodes.len() {
            if g.nodes[i].of.is_some() || !matches!(cols[i], 1 | 2) {
                continue;
            }
            if inside(&cols, i) > 0 {
                cols[i] = 3 - cols[i];
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    for (i, n) in g.nodes.iter().enumerate() {
        if let Some(of) = n.of {
            cols[i] = cols[of];
        }
    }
    cols
}

/// A cubic's point at `t`.
fn bezier(p: [(f64, f64); 4], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let w = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
    (
        (0..4).map(|i| w[i] * p[i].0).sum(),
        (0..4).map(|i| w[i] * p[i].1).sum(),
    )
}

fn label_width(e: &Edge) -> f64 {
    let chars = e.label.chars().count() + e.lost.as_ref().map_or(0, |l| l.chars().count() + 3);
    chars as f64 * 6.4 + 18.0
}

/// Where everything sits: the boxes in three columns, each ordered to keep
/// its lines short, a bridge's ASIO side right under it; the lines as
/// curves between the columns, as arcs inside one, as a loop for a device
/// subscribed to itself; each label on its line where it covers no box and
/// no other label.
pub fn layout(g: &Graph) -> Placed {
    // Places set and a device there to stand in one: the columns are the
    // places. Else, as ever, who sends and who receives.
    let placed = g
        .places
        .iter()
        .any(|(d, _)| g.nodes.iter().any(|n| n.name == *d));
    let (cols, xs, names) = if placed {
        by_place(g)
    } else {
        (columns(g), COL_X.to_vec(), Vec::new())
    };
    let width = if placed {
        (PLACE_EDGE * 2.0 + BOX_W + (xs.len() - 1) as f64 * PLACE_PITCH).max(900.0)
    } else {
        WIDTH
    };
    let lower = |i: usize| g.nodes[i].name.to_lowercase();
    // A column's order: its devices (an ASIO side follows its bridge).
    let mut order: Vec<Vec<usize>> = vec![Vec::new(); xs.len()];
    for (i, n) in g.nodes.iter().enumerate() {
        if n.kind != NodeKind::Asio {
            order[cols[i]].push(i);
        }
    }
    // With places: a place's devices stand as the owner put them (the
    // order `places` came in); only "no place set" is ordered by the map.
    let rank = |i: usize| {
        g.places
            .iter()
            .position(|(d, _)| *d == g.nodes[i].name)
            .unwrap_or(usize::MAX)
    };
    for col in &mut order {
        col.sort_by_key(|&i| (rank(i), lower(i)));
    }
    let asio_of = |i: usize| g.nodes.iter().position(|n| n.of == Some(i));
    let place = |order: &[Vec<usize>]| -> (Vec<Rect>, f64) {
        let mut rects = vec![
            Rect {
                x: 0.0,
                y: 0.0,
                w: BOX_W,
                h: BOX_H
            };
            g.nodes.len()
        ];
        let mut heights = vec![0.0f64; order.len()];
        for (c, col) in order.iter().enumerate() {
            let mut y = 0.0;
            for &i in col {
                rects[i] = Rect {
                    x: xs[c],
                    y,
                    w: BOX_W,
                    h: BOX_H,
                };
                y += BOX_H + GAP;
                if let Some(a) = asio_of(i) {
                    y += ASIO_GAP - GAP;
                    rects[a] = Rect {
                        x: xs[c],
                        y,
                        w: BOX_W,
                        h: BOX_H,
                    };
                    y += BOX_H + GAP;
                }
            }
            heights[c] = (y - GAP).max(0.0);
        }
        let tallest = heights.iter().copied().fold(BOX_H, f64::max);
        for (i, r) in rects.iter_mut().enumerate() {
            r.y += TOP + (tallest - heights[cols[i]]) / 2.0;
        }
        (rects, TOP + tallest + 24.0)
    };
    let (mut rects, mut height) = place(&order);
    // Each column in turn by the mean height of its devices' partners in
    // the other columns: connected devices end up at the same height.
    let turns: Vec<usize> = if placed {
        (0..xs.len()).filter(|&c| names[c] == NO_PLACE).collect()
    } else {
        vec![1, 2, 0, 3]
    };
    for _ in 0..3 {
        for &c in &turns {
            let key = |i: usize| -> f64 {
                let partners: Vec<f64> = g
                    .edges
                    .iter()
                    .filter(|e| matches!(e.sel, Sel::Link { .. }))
                    .filter_map(|e| match (e.from == i, e.to == i) {
                        (true, false) => Some(e.to),
                        (false, true) => Some(e.from),
                        _ => None,
                    })
                    .filter(|&o| cols[o] != c)
                    .map(|o| rects[o].mid_y())
                    .collect();
                if partners.is_empty() {
                    rects[i].mid_y()
                } else {
                    partners.iter().sum::<f64>() / partners.len() as f64
                }
            };
            let mut keyed: Vec<(f64, String, usize)> =
                order[c].iter().map(|&i| (key(i), lower(i), i)).collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            order[c] = keyed.into_iter().map(|k| k.2).collect();
            (rects, height) = place(&order);
        }
    }
    // The sides a line leaves and enters by.
    #[derive(Clone, Copy, PartialEq)]
    enum Side {
        Left,
        Right,
    }
    let sides = |e: &Edge| -> Option<(Side, Side)> {
        if !matches!(e.sel, Sel::Link { .. } | Sel::Video { .. }) || e.from == e.to {
            return None;
        }
        Some(match cols[e.from].cmp(&cols[e.to]) {
            std::cmp::Ordering::Less => (Side::Right, Side::Left),
            std::cmp::Ordering::Greater => (Side::Left, Side::Right),
            std::cmp::Ordering::Equal if rects[e.from].y < rects[e.to].y => {
                (Side::Right, Side::Right)
            }
            std::cmp::Ordering::Equal => (Side::Left, Side::Left),
        })
    };
    // A line's height on a box's side: the side's lines spread out, in the
    // order of their other ends.
    let port = |node: usize, side: Side, edge: usize| -> f64 {
        let mut here: Vec<(f64, usize)> = g
            .edges
            .iter()
            .enumerate()
            .filter_map(|(k, e)| {
                let (a, z) = sides(e)?;
                if e.from == node && a == side {
                    Some((rects[e.to].mid_y(), k))
                } else if e.to == node && z == side {
                    Some((rects[e.from].mid_y(), k))
                } else {
                    None
                }
            })
            .collect();
        here.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let at = here.iter().position(|&(_, k)| k == edge).unwrap_or(0);
        rects[node].y + rects[node].h * (at + 1) as f64 / (here.len() + 1) as f64
    };
    // Per line: its path, the places its label may sit, the label's width.
    type Path = (String, Vec<(f64, f64)>, f64);
    let mut paths: Vec<Path> = Vec::with_capacity(g.edges.len());
    for (k, e) in g.edges.iter().enumerate() {
        let (a, z) = (rects[e.from], rects[e.to]);
        let lw = label_width(e);
        // The path, and where its label may sit, best first.
        let (d, spots): (String, Vec<(f64, f64)>) = if let Sel::Bridge { to_dante, .. } = e.sel {
            // Two short vertical lines between the bridge and its ASIO side.
            let (dev, asio) = if to_dante { (z, a) } else { (a, z) };
            let x = dev.x + dev.w * if to_dante { 0.27 } else { 0.73 };
            let (y1, y2) = if to_dante {
                (asio.y, dev.y + dev.h)
            } else {
                (dev.y + dev.h, asio.y)
            };
            (
                format!("M{x:.1},{y1:.1} L{x:.1},{y2:.1}"),
                // One label above the other: a count of underruns is wide.
                vec![(x, (y1 + y2) / 2.0 + if to_dante { -12.0 } else { 12.0 })],
            )
        } else if e.from == e.to {
            let (x, y) = (a.x + a.w, a.mid_y());
            (
                format!(
                    "M{x:.1},{:.1} C{:.1},{:.1} {:.1},{:.1} {x:.1},{:.1}",
                    y - 12.0,
                    x + 64.0,
                    y - 34.0,
                    x + 64.0,
                    y + 34.0,
                    y + 12.0
                ),
                vec![(x + 48.0 + lw / 2.0, y)],
            )
        } else {
            let (sa, sz) = sides(e).unwrap_or((Side::Right, Side::Left));
            let x1 = if sa == Side::Right { a.x + a.w } else { a.x };
            let x2 = if sz == Side::Right { z.x + z.w } else { z.x };
            let (y1, y2) = (port(e.from, sa, k), port(e.to, sz, k));
            let p = if sa == sz {
                // Inside one column: an arc out of the side and back.
                let out = (36.0 + (y2 - y1).abs() * 0.25).min(110.0)
                    * if sa == Side::Right { 1.0 } else { -1.0 };
                [(x1, y1), (x1 + out, y1), (x2 + out, y2), (x2, y2)]
            } else {
                let bend = ((x2 - x1).abs() * 0.45).max(60.0) * (x2 - x1).signum();
                [(x1, y1), (x1 + bend, y1), (x2 - bend, y2), (x2, y2)]
            };
            (
                format!(
                    "M{:.1},{:.1} C{:.1},{:.1} {:.1},{:.1} {:.1},{:.1}",
                    p[0].0, p[0].1, p[1].0, p[1].1, p[2].0, p[2].1, p[3].0, p[3].1
                ),
                // A line that crosses a column has fewer free places: try
                // along all of it.
                [
                    0.5, 0.36, 0.64, 0.26, 0.74, 0.18, 0.82, 0.44, 0.56, 0.31, 0.69, 0.12, 0.88,
                ]
                .iter()
                .map(|&t| bezier(p, t))
                .collect(),
            )
        };
        paths.push((d, spots, lw));
    }
    // The labels, the widest first: a wide one (late packets counted) has
    // the fewest places between two columns, a short one fits anywhere.
    let mut by_width: Vec<usize> = (0..paths.len()).collect();
    by_width.sort_by(|&a, &b| paths[b].2.total_cmp(&paths[a].2).then(a.cmp(&b)));
    let mut labels: Vec<Option<Rect>> = vec![None; paths.len()];
    for k in by_width {
        let (_, spots, lw) = &paths[k];
        // A label stays on the schema: an arc at the edge has its own
        // beside it, not half past the edge.
        let boxed = |(x, y): (f64, f64)| Rect {
            x: (x - lw / 2.0).clamp(4.0, (width - lw - 4.0).max(4.0)),
            y: y - LABEL_H / 2.0,
            w: *lw,
            h: LABEL_H,
        };
        let free = |r: &Rect| {
            !rects.iter().any(|n| n.hits(r)) && !labels.iter().flatten().any(|o| o.hits(r))
        };
        // On the line first; with every place there taken, beside it.
        let label = [0.0, 22.0, -22.0, 44.0, -44.0, 66.0, -66.0, 88.0, -88.0]
            .iter()
            .flat_map(|&dy| spots.iter().map(move |&(x, y)| boxed((x, y + dy))))
            .find(free)
            .unwrap_or_else(|| boxed(spots[0]));
        labels[k] = Some(label);
    }
    let edges: Vec<Geo> = paths
        .into_iter()
        .zip(labels)
        .map(|((d, _, _), label)| Geo {
            d,
            label: label.expect("every line has its label"),
        })
        .collect();
    let (titles, areas) = if placed {
        (
            names
                .into_iter()
                .enumerate()
                .map(|(c, n)| (xs[c] - 8.0, n))
                .collect(),
            xs.iter()
                .map(|&x| Rect {
                    x: x - 20.0,
                    y: 4.0,
                    w: BOX_W + 40.0,
                    h: height - 8.0,
                })
                .collect(),
        )
    } else {
        // The two middle columns share one title, over the first in use.
        let middle = if cols.contains(&1) { 1 } else { 2 };
        let titles = [
            "sends",
            "sends and receives",
            "sends and receives",
            "receives",
        ]
        .iter()
        .enumerate()
        .filter(|(i, _)| cols.contains(i) && (!matches!(i, 1 | 2) || *i == middle))
        .map(|(i, t)| (COL_X[i], (*t).to_owned()))
        .collect();
        (titles, Vec::new())
    };
    Placed {
        width,
        height,
        nodes: rects,
        cols,
        edges,
        titles,
        areas,
    }
}

// ---- drawing ----

/// `text` safe inside SVG text and attributes.
pub fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `name` cut to what a box holds.
fn short(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        name.to_owned()
    } else {
        let cut: String = name.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

/// More connections than this and the lines are drawn thinner. Every line
/// keeps its latency (owner, 21.11.2026); "Problems only" dims the good ones.
pub const QUIET_OVER: usize = 12;

/// The schema as SVG: a box per node (`data-node`), a line per connection
/// (`data-edge`: its hash), `chosen` standing out with its two ends.
#[cfg(test)]
pub fn svg(g: &Graph, p: &Placed, chosen: Option<&Sel>) -> String {
    svg_with(g, p, chosen, &Show::default())
}

/// The schema with what the owner switched on: only the problems bright,
/// a device looked for marked.
pub fn svg_with(g: &Graph, p: &Placed, chosen: Option<&Sel>, show: &Show) -> String {
    let good = |e: &Edge| matches!(e.state, State::Ok | State::Unknown);
    // A device with no connection in trouble is calm.
    let calm = |i: usize| {
        !g.edges
            .iter()
            .any(|e| !good(e) && (e.from == i || e.to == i))
    };
    let hit = |n: &Node| !show.find.is_empty() && n.name.to_lowercase().contains(&show.find);
    let on = |e: &Edge| chosen.is_some_and(|c| c.same(&e.sel));
    // A chosen device: its box, and its connections brought forward.
    let device = chosen.and_then(|c| match c {
        Sel::Device { name } => node_of(g, name),
        _ => None,
    });
    let dense = g.edges.len() > QUIET_OVER;
    let mut s = format!(
        r#"<svg class="map{}{}{}{}" viewBox="0 0 {:.0} {:.0}" role="img" aria-label="Devices and their connections">"#,
        if chosen.is_some() { " has-sel" } else { "" },
        if device.is_some() { " has-device" } else { "" },
        if dense { " dense" } else { "" },
        if show.problems { " problems" } else { "" },
        p.width,
        p.height
    );
    s.push_str("<defs>");
    for c in ["ok", "warn", "bad", "unknown"] {
        s.push_str(&format!(
            r#"<marker id="map-arrow-{c}" class="arrow {c}" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0,0 L10,5 L0,10 z"/></marker>"#
        ));
    }
    s.push_str("</defs>");
    // A place's name as the page's script reads it off an area or a box
    // (`mapdrag.js`): empty for "no place set".
    let place_at = |col: usize| {
        p.titles
            .get(col)
            .map(|(_, n)| n.as_str())
            .filter(|n| !p.areas.is_empty() && *n != NO_PLACE)
            .unwrap_or("")
    };
    for (c, r) in p.areas.iter().enumerate() {
        s.push_str(&format!(
            r#"<rect class="area" data-testid="map-area" data-place="{}" x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="14"/>"#,
            esc(place_at(c)),
            r.x,
            r.y,
            r.w,
            r.h
        ));
    }
    for (c, (x, title)) in p.titles.iter().enumerate() {
        // A place's title is its handle: dragged, it moves the place among
        // the places (`mapdrag.js`).
        let handle = match place_at(c) {
            "" => String::new(),
            name => format!(r#" data-place="{}""#, esc(name)),
        };
        s.push_str(&format!(
            r#"<text class="col-title"{handle} x="{x}" y="22">{}</text>"#,
            esc(title)
        ));
    }
    let of = |e: &Edge| device.is_some_and(|d| e.from == d || e.to == d);
    let ends: Vec<usize> = g
        .edges
        .iter()
        .filter(|e| on(e) || of(e))
        .flat_map(|e| [e.from, e.to])
        .collect();
    // The chosen line last: on top of the others.
    let mut order: Vec<usize> = (0..g.edges.len()).collect();
    order.sort_by_key(|&k| on(&g.edges[k]));
    for k in order {
        let (e, geo) = (&g.edges[k], &p.edges[k]);
        let class = e.state.class();
        let (from, to) = (&g.nodes[e.from].name, &g.nodes[e.to].name);
        let l = geo.label;
        let lost = e.lost.as_ref().map_or(String::new(), |t| {
            format!(r#"<tspan class="lost"> · {}</tspan>"#, esc(t))
        });
        s.push_str(&format!(
            r#"<g class="edge {class}{}{}{}{}" data-testid="map-edge" data-edge="{}" data-from="{}" data-to="{}" data-state="{class}"><path class="line" d="{}" marker-end="url(#map-arrow-{class})"/><g class="lbl"><rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.0}" rx="10"/><text x="{:.1}" y="{:.1}">{}{lost}</text></g><path class="hit" d="{}"><title>{}</title></path></g>"#,
            if on(e) { " sel" } else { "" },
            if of(e) { " of" } else { "" },
            if show.problems && good(e) {
                " quiet"
            } else {
                ""
            },
            // A video line is drawn dashed: another kind of signal (#124).
            if matches!(e.sel, Sel::Video { .. }) {
                " video"
            } else {
                ""
            },
            esc(&e.sel.hash()),
            esc(from),
            esc(to),
            geo.d,
            l.x,
            l.y,
            l.w,
            l.h,
            l.x + l.w / 2.0,
            l.y + 14.0,
            esc(&e.label),
            geo.d,
            esc(&e.title),
        ));
    }
    for (i, n) in g.nodes.iter().enumerate() {
        let r = p.nodes[i];
        let kind = match n.kind {
            NodeKind::Device => "",
            NodeKind::Missing => " missing",
            NodeKind::Asio => " asio",
            NodeKind::Video => " video",
        };
        let badge = n.badge.as_ref().map_or(String::new(), |(text, good)| {
            format!(
                r#"<text class="badge{}" x="{:.1}" y="{:.1}" text-anchor="end">{}</text>"#,
                if *good { "" } else { " bad" },
                r.x + r.w - 10.0,
                r.y + 20.0,
                esc(text)
            )
        });
        let dsync = n.dsync.as_ref().map_or(String::new(), |(text, good)| {
            format!(
                r#"<text class="badge{}" data-testid="map-node-dsync" x="{:.1}" y="{:.1}" text-anchor="end">{}</text>"#,
                if *good { "" } else { " bad" },
                r.x + r.w - 10.0,
                r.y + 52.0,
                esc(text)
            )
        });
        let room = if n.badge.is_some() { 17 } else { 24 };
        s.push_str(&format!(
            r#"<g class="node{kind}{}{}{}" data-testid="map-node" data-node="{}" data-place="{}"><title>{}</title><rect x="{:.1}" y="{:.1}" width="{:.0}" height="{:.0}" rx="10"/><text class="name" x="{:.1}" y="{:.1}">{}</text><text class="sub" x="{:.1}" y="{:.1}">{}</text><text class="sub" x="{:.1}" y="{:.1}">{}</text>{badge}{dsync}</g>"#,
            if device == Some(i) {
                " sel"
            } else if ends.contains(&i) {
                " rel"
            } else {
                ""
            },
            if show.problems && calm(i) { " calm" } else { "" },
            if hit(n) { " hit" } else { "" },
            esc(&n.name),
            esc(place_at(p.cols[i])),
            esc(&n.name),
            r.x,
            r.y,
            r.w,
            r.h,
            r.x + 12.0,
            r.y + 22.0,
            esc(&short(&n.name, room)),
            r.x + 12.0,
            r.y + 39.0,
            esc(&short(&n.lines[0], 32)),
            r.x + 12.0,
            r.y + 52.0,
            esc(&short(&n.lines[1], 32)),
        ));
    }
    s.push_str("</svg>");
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn video_connections_are_lines_of_their_own_kind_coloured_by_frames_lost() {
        use dcontroller_api::{VideoConn, VideoNode, VideoReport};
        let mut g = build(&net(), None, Span::H1);
        let dante = g
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Device)
            .unwrap()
            .name
            .clone();
        let conn = |id: &str, source: &str, sink: &str| VideoConn {
            id: id.into(),
            node: sink.into(),
            source: source.into(),
            sink: sink.into(),
            kind: "jxs".into(),
            direction: "in".into(),
            latency_ms: Some(41.0),
            ..VideoConn::default()
        };
        let v = VideoReport {
            nodes: vec![VideoNode {
                name: "cam3".into(),
                kind: "cambox".into(),
                ..VideoNode::default()
            }],
            connections: vec![
                VideoConn {
                    lost_1h: 2,
                    lost_24h: 5,
                    ..conn("a", "cam3", "strih")
                },
                VideoConn {
                    duplicated_1h: 1,
                    ..conn("b", "cam3", &dante.to_lowercase())
                },
                VideoConn {
                    lost_24h: 4,
                    ..conn("c", "cam4", "strih")
                },
            ],
            ..VideoReport::default()
        };
        let before = g.nodes.len();
        with_video(&mut g, &v, Span::H1);
        // cam3, strih and cam4 are new boxes; the Dante device stays one box.
        assert_eq!(g.nodes.len(), before + 3);
        let cam3 = g.nodes.iter().find(|n| n.name == "cam3").unwrap();
        assert_eq!(
            (cam3.kind, cam3.lines[0].as_str()),
            (NodeKind::Video, "video · cambox")
        );
        let e = |id: &str| {
            g.edges
                .iter()
                .find(|e| e.sel == Sel::Video { id: id.into() })
                .unwrap()
        };
        assert_eq!(
            (e("a").state, e("a").label.as_str(), e("a").lost.as_deref()),
            (State::Bad, "41 ms", Some("2 lost"))
        );
        assert_eq!(e("b").state, State::Warn);
        assert_eq!(g.nodes[e("b").to].kind, NodeKind::Device);
        // Lost a day ago only: green in the hour, red in the day's window.
        assert_eq!(e("c").state, State::Ok);
        let mut day = build(&net(), None, Span::H24);
        with_video(&mut day, &v, Span::H24);
        assert_eq!(
            day.edges
                .iter()
                .find(|e| e.sel == Sel::Video { id: "c".into() })
                .unwrap()
                .state,
            State::Bad
        );
        // Its hash names it, and it counts with the rig's connections.
        assert_eq!(
            Sel::from_hash(&e("a").sel.hash()),
            Some(Sel::Video { id: "a".into() })
        );
        assert_eq!(
            Sel::from_hash("#map/video/jxs%3Acam3-%3Estrih"),
            Some(Sel::Video {
                id: "jxs:cam3->strih".into()
            })
        );
        let h = health(&g);
        assert_eq!(h.lost_video, [("cam3 → strih".to_owned(), 2)]);
        let s = svg(&g, &layout(&g), None);
        assert!(s.contains(r#" video" data-testid="map-edge""#));
        assert!(s.contains(r#"class="node video"#));
    }

    #[test]
    fn a_devices_box_carries_its_own_dsync_clock() {
        use dcontroller_api::{ClockNode, Clocks, Follower};
        let mut g = build(&net(), None, Span::H1);
        let name = g
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::Device)
            .unwrap()
            .name
            .clone();
        let locked = ClockNode {
            device: name.to_uppercase(),
            ready: true,
            source: "follower".into(),
            follower: Some(Follower {
                locked: true,
                settled: true,
                ..Follower::default()
            }),
            ..ClockNode::default()
        };
        with_dsync(
            &mut g,
            &Clocks {
                nodes: vec![locked.clone()],
                ..Clocks::default()
            },
        );
        let n = g.nodes.iter().find(|n| n.name == name).unwrap();
        assert_eq!(n.dsync, Some(("dsync locked".to_owned(), true)));
        assert!(
            g.nodes
                .iter()
                .filter(|n| n.name != name)
                .all(|n| n.dsync.is_none())
        );
        let s = svg(&g, &layout(&g), None);
        assert!(s.contains(r#"data-testid="map-node-dsync""#));
        assert!(s.contains(">dsync locked</text>"));
        let lost = ClockNode {
            ready: false,
            ..locked
        };
        with_dsync(
            &mut g,
            &Clocks {
                nodes: vec![lost],
                ..Clocks::default()
            },
        );
        let n = g.nodes.iter().find(|n| n.name == name).unwrap();
        assert_eq!(n.dsync, Some(("dsync no time".to_owned(), false)));
    }

    #[test]
    fn an_old_pages_link_lands_on_the_map() {
        assert_eq!(moved("#overview").as_deref(), Some("#map"));
        assert_eq!(moved("#devices").as_deref(), Some("#map/devices"));
        assert_eq!(moved("#devices/").as_deref(), Some("#map/devices"));
        assert_eq!(
            moved("#devices/DESK%20A").as_deref(),
            Some("#map/device/DESK%20A")
        );
        assert_eq!(moved("#latency/amp").as_deref(), Some("#map/device/amp"));
        assert_eq!(moved("#devices/%zz").as_deref(), Some("#map/devices"));
        for kept in ["#map", "#routing", "#map/device/a", "", "#changes/a"] {
            assert_eq!(moved(kept), None, "{kept}");
        }
        assert_eq!(tab_of("#map/devices"), Some("devices"));
        assert_eq!(tab_of("#map/links"), Some("links"));
        assert_eq!(tab_of("#map/schema"), Some("schema"));
        assert_eq!(tab_of("#map/device/a"), None);
        // A tab's hash chooses nothing.
        assert_eq!(Sel::from_hash("#map/devices"), None);
    }

    use super::*;
    use dcontroller_api::{
        BridgePoint, BridgeSummary, Channels, Clock, Device, RxChannel, Source, TxChannel,
    };

    fn dev(name: &str, tx: usize, rx: &[Option<&str>]) -> Device {
        Device {
            name: name.into(),
            channels: Some(Channels {
                tx: (1..=tx)
                    .map(|c| TxChannel {
                        channel: c as u16,
                        label: format!("{c:02}"),
                    })
                    .collect(),
                rx: rx
                    .iter()
                    .enumerate()
                    .map(|(i, s)| RxChannel {
                        channel: i as u16 + 1,
                        label: format!("{:02}", i + 1),
                        source: s.map(|d| Source {
                            channel: "01".into(),
                            device: d.into(),
                        }),
                        ..RxChannel::default()
                    })
                    .collect(),
            }),
            ..Device::default()
        }
    }

    fn net() -> Devices {
        Devices {
            devices: vec![
                dev("amp", 0, &[Some("desk"), Some("desk")]),
                dev("bridge", 2, &[Some("player"), Some("PLAYER")]),
                dev("desk", 4, &[Some("player"), Some("stage"), None]),
                dev("idle", 2, &[None]),
                dev("player", 2, &[]),
                dev("stage", 4, &[Some("desk"), Some("gone")]),
                dev("stream", 0, &[Some("bridge")]),
            ],
            ..Devices::default()
        }
    }

    fn sum(from: &str, to: &str, lat: u32, peak: u32, late_1h: u64, late_24h: u64) -> LinkSummary {
        LinkSummary {
            from: from.into(),
            to: to.into(),
            latency_us: lat,
            peak_us: peak,
            late_1h,
            late_24h,
            ..LinkSummary::default()
        }
    }

    fn links() -> Links {
        Links {
            now: 0,
            links: vec![
                sum("Player", "bridge", 8360, 13_280, 67_004, 131_870),
                sum("player", "desk", 1000, 410, 0, 0),
                sum("desk", "amp", 1000, 950, 0, 0),
                sum("bridge", "stream", 13_040, 4100, 0, 2),
            ],
            bridges: vec![BridgeSummary {
                device: "BRIDGE".into(),
                last: BridgePoint {
                    card_asio_to_dante_us: Some(7300),
                    card_dante_to_asio_us: Some(7500),
                    driver: Some("SoundGrid".into()),
                    ..BridgePoint::default()
                },
                to_asio_underruns_1h: 2,
                from_asio_underruns_1h: 0,
                to_asio_underruns_24h: 5,
                from_asio_underruns_24h: 1,
            }],
        }
    }

    #[test]
    fn a_days_underruns_sum_every_bridge_both_ways() {
        assert_eq!(underruns_24h(&links()), 6);
        assert_eq!(underruns_24h(&Links::default()), 0);
    }

    fn edge<'a>(g: &'a Graph, from: &str, to: &str) -> &'a Edge {
        g.edges
            .iter()
            .find(|e| g.nodes[e.from].name == from && g.nodes[e.to].name == to)
            .unwrap_or_else(|| panic!("no edge {from} → {to}"))
    }

    #[test]
    fn a_choice_and_its_hash_name_each_other() {
        let l = Sel::Link {
            from: "desk a/b".into(),
            to: "amp-1".into(),
        };
        assert_eq!(l.hash(), "#map/link/desk%20a%2Fb/amp-1");
        assert_eq!(Sel::from_hash(&l.hash()), Some(l.clone()));
        let b = Sel::Bridge {
            device: "dsg".into(),
            to_dante: false,
        };
        assert_eq!(b.hash(), "#map/bridge/dsg/to-asio");
        assert_eq!(Sel::from_hash(&b.hash()), Some(b.clone()));
        assert_eq!(l.device(), Some("amp-1"));
        assert_eq!(b.device(), Some("dsg"));
        let d = Sel::Device {
            name: "stage rack".into(),
        };
        assert_eq!(d.hash(), "#map/device/stage%20rack");
        assert_eq!(Sel::from_hash(&d.hash()), Some(d.clone()));
        assert_eq!(d.device(), Some("stage rack"));
        assert!(d.same(&Sel::Device {
            name: "STAGE RACK".into()
        }));
        assert!(!d.same(&l));
        assert_eq!(Sel::from_hash("#map/device/a/b"), None);
        for bad in [
            "#map",
            "#map/link/a",
            "#map/bridge/a/up",
            "#map/link/a/b/c",
            "#map/link/%zz/b",
            "#devices/a",
        ] {
            assert_eq!(Sel::from_hash(bad), None, "{bad}");
        }
        assert!(l.same(&Sel::Link {
            from: "DESK A/B".into(),
            to: "Amp-1".into()
        }));
        assert!(!l.same(&b));
        assert_eq!(decode(&encode("zvuk ô→")).as_deref(), Some("zvuk ô→"));
    }

    #[test]
    fn the_graph_has_a_line_per_subscribed_pair_with_its_figures() {
        let l = links();
        let g = build(&net(), Some(&l), Span::H1);
        // 7 devices, the source that is gone, the bridge's ASIO side.
        assert_eq!(g.nodes.len(), 9);
        assert_eq!(g.nodes[7].name, "gone");
        assert_eq!(g.nodes[7].kind, NodeKind::Missing);
        assert_eq!(g.nodes[8].name, "SoundGrid");
        assert_eq!(g.nodes[8].of, Some(1));
        // Two channels from one source are one line, the names' case aside.
        let bad = edge(&g, "player", "bridge");
        assert_eq!(bad.state, State::Bad);
        assert_eq!(bad.label, "8.36 ms");
        assert_eq!(bad.lost.as_deref(), Some("67 004 late"));
        assert!(bad.title.contains("2 channels"));
        assert_eq!(edge(&g, "player", "desk").state, State::Ok);
        // A peak at 95 % of the latency is amber; late packets earlier
        // than the hour are no colour in the hour (#142)...
        assert_eq!(edge(&g, "desk", "amp").state, State::Warn);
        assert_eq!(edge(&g, "bridge", "stream").state, State::Ok);
        // ...and red in the 24 h window, which counts them.
        let day = build(&net(), Some(&l), Span::H24);
        let e = edge(&day, "bridge", "stream");
        assert_eq!((e.state, e.lost.as_deref()), (State::Bad, Some("2 late")));
        // Not recorded: no figures, the line is still there.
        let unknown = edge(&g, "desk", "stage");
        assert_eq!(unknown.state, State::Unknown);
        assert_eq!(unknown.label, "–");
        assert_eq!(edge(&g, "gone", "stage").state, State::Unknown);
        // The bridge's two delays, named by its driver.
        let up = edge(&g, "SoundGrid", "bridge");
        assert_eq!(
            up.sel,
            Sel::Bridge {
                device: "bridge".into(),
                to_dante: true
            }
        );
        assert_eq!((up.label.as_str(), up.state), ("7.3 ms", State::Ok));
        let down = edge(&g, "bridge", "SoundGrid");
        assert_eq!((down.label.as_str(), down.state), ("7.5 ms", State::Bad));
        assert_eq!(down.lost.as_deref(), Some("2 underruns"));
        assert_eq!(g.edges.len(), 9);
        // All of them, the worst first; and the network's hour in one line.
        let order: Vec<State> = worst_first(&g).iter().map(|e| e.state).collect();
        assert!(order.windows(2).all(|w| w[0] >= w[1]), "{order:?}");
        assert_eq!(worst_first(&g)[0].sel, bad.sel);
        assert_eq!(worst_first(&g).len(), g.edges.len());
        let h = health(&g);
        assert_eq!((h.links, h.measured, h.clean, h.warn), (7, 4, 3, 1));
        assert_eq!(h.lost, [("player → bridge".to_owned(), 67_004)]);
        assert_eq!((h.bridges, h.underruns), (1, 2));
        assert_eq!(health(&Graph::default()), Health::default());
        // The worst shows when nothing is chosen.
        assert_eq!(worst(&g).map(|e| &e.sel), Some(&bad.sel));
        assert!(
            edge_of(
                &g,
                &Sel::Link {
                    from: "PLAYER".into(),
                    to: "Bridge".into()
                }
            )
            .is_some()
        );
        // Without summaries every line is unknown; no bridge side.
        let plain = build(&net(), None, Span::H1);
        assert_eq!(plain.nodes.len(), 8);
        assert!(plain.edges.iter().all(|e| e.state == State::Unknown));
        assert_eq!(worst(&Graph::default()), None);
    }

    #[test]
    fn devices_sit_in_columns_by_their_part_with_no_overlap() {
        let l = links();
        let g = build(&net(), Some(&l), Span::H1);
        let p = layout(&g);
        let col = |name: &str| p.cols[g.nodes.iter().position(|n| n.name == name).unwrap()];
        assert_eq!(col("player"), 0);
        assert_eq!(col("gone"), 0);
        // Sending and receiving: one of the two middle columns.
        for name in ["desk", "stage", "bridge"] {
            assert!(matches!(col(name), 1 | 2), "{name} in the middle");
        }
        assert_eq!(col("SoundGrid"), col("bridge"));
        assert_eq!(col("amp"), 3);
        assert_eq!(col("stream"), 3);
        // Not connected: by its channels (TX and RX: the middle).
        assert_eq!(col("idle"), 1);
        for (i, a) in p.nodes.iter().enumerate() {
            assert!(a.y >= TOP && a.y + a.h <= p.height, "{i} inside");
            for b in &p.nodes[i + 1..] {
                assert!(!a.hits(b), "boxes overlap");
            }
        }
        // The ASIO side sits right under its bridge.
        let (b, a) = (p.nodes[1], p.nodes[8]);
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y + b.h + ASIO_GAP);
        // Every label is on the canvas, off the boxes and off each other.
        for (i, e) in p.edges.iter().enumerate() {
            assert!(
                e.label.x >= 0.0 && e.label.x + e.label.w <= WIDTH,
                "label {i} on canvas"
            );
            assert!(
                !p.nodes.iter().any(|n| n.hits(&e.label)),
                "label {i} on a box"
            );
            for o in &p.edges[i + 1..] {
                assert!(!e.label.hits(&o.label), "labels overlap");
            }
        }
        // The same network lays out the same way every time.
        assert_eq!(layout(&g), p);
    }

    #[test]
    fn problems_only_dims_the_good_lines_and_the_calm_devices() {
        // 14 receivers of one source: more lines than a schema can label.
        let mut devices = vec![dev("src", 2, &[])];
        for i in 0..14 {
            devices.push(dev(&format!("rx{i:02}"), 0, &[Some("src")]));
        }
        let d = Devices {
            devices,
            ..Devices::default()
        };
        let l = Links {
            links: vec![
                sum("src", "rx00", 1000, 2000, 5, 5),
                sum("src", "rx01", 1000, 100, 0, 0),
            ],
            ..Links::default()
        };
        let g = build(&d, Some(&l), Span::H1);
        assert_eq!(g.edges.len(), 14);
        let s = svg(&g, &layout(&g), None);
        assert!(s.starts_with(r#"<svg class="map dense" "#));
        // Every line is drawn whole until the owner asks for the problems.
        assert!(!s.contains(" quiet") && !s.contains(" calm") && !s.contains(" hit"));
        let show = Show {
            problems: true,
            find: "RX0".to_lowercase(),
        };
        let s = svg_with(&g, &layout(&g), None, &show);
        assert!(s.starts_with(r#"<svg class="map dense problems" "#));
        assert!(s.contains(r#"class="edge bad" data-testid"#));
        assert_eq!(s.matches(r#"class="edge ok quiet""#).count(), 1);
        assert_eq!(s.matches(r#"class="edge unknown quiet""#).count(), 12);
        // The source and the receiver losing audio stay; 13 devices are calm.
        assert_eq!(s.matches(" calm").count(), 13);
        // rx00 to rx09 carry the name looked for.
        assert_eq!(s.matches(" hit\"").count(), 10);
    }

    #[test]
    fn with_places_set_the_devices_stand_in_their_places() {
        let d = Devices {
            devices: vec![
                dev("rack", 2, &[]),
                dev("desk", 2, &[Some("rack")]),
                dev("amp", 0, &[Some("desk")]),
                dev("spare", 0, &[]),
            ],
            ..Devices::default()
        };
        let mut g = build(&d, None, Span::H1);
        // No place set: the columns of who sends and who receives.
        let p = layout(&g);
        assert_eq!(p.width, WIDTH);
        assert!(p.areas.is_empty());
        assert_eq!(p.titles[0].1, "sends");
        // A place of a device that is not there changes nothing.
        g.places = vec![("gone".into(), "Attic".into())];
        assert!(layout(&g).areas.is_empty());
        g.places = vec![
            ("desk".into(), "FOH".into()),
            ("rack".into(), "Stage".into()),
            ("amp".into(), "FOH".into()),
            ("gone".into(), "Attic".into()),
        ];
        let p = layout(&g);
        let titles: Vec<&str> = p.titles.iter().map(|t| t.1.as_str()).collect();
        // The places as first used; no area for a place with no device.
        assert_eq!(titles, ["FOH", "Stage", NO_PLACE]);
        let col = |name: &str| p.cols[g.nodes.iter().position(|n| n.name == name).unwrap()];
        assert_eq!(
            (col("desk"), col("amp"), col("rack"), col("spare")),
            (0, 0, 1, 2)
        );
        assert_eq!(p.areas.len(), 3);
        for (i, r) in p.nodes.iter().enumerate() {
            let a = p.areas[p.cols[i]];
            assert!(
                r.x >= a.x && r.x + r.w <= a.x + a.w && r.y >= a.y && r.y + r.h <= a.y + a.h,
                "{} inside its place",
                g.nodes[i].name
            );
        }
        for e in &p.edges {
            assert!(e.label.x >= 0.0 && e.label.x + e.label.w <= p.width);
        }
        // Inside a place the devices stand as the owner put them.
        let y = |p: &Placed, name: &str| {
            p.nodes[g.nodes.iter().position(|n| n.name == name).unwrap()].y
        };
        assert!(y(&p, "desk") < y(&p, "amp"));
        let mut turned = g.clone();
        turned.places.swap(0, 2);
        let t = layout(&turned);
        assert!(y(&t, "amp") < y(&t, "desk"));
        let s = svg(&g, &p, None);
        // The script that drags reads the place off an area and a box.
        assert!(s.contains(r#"data-testid="map-area" data-place="FOH""#));
        assert!(s.contains(r#"data-testid="map-area" data-place="""#));
        assert!(s.contains(r#"data-node="amp" data-place="FOH""#));
        assert!(s.contains(r#"data-node="spare" data-place="""#));
        // A place's title is the handle to move it; "no place set" has none.
        assert!(s.contains(r#"<text class="col-title" data-place="Stage""#));
        assert!(s.contains(r#"<text class="col-title" x="#));
        assert_eq!(s.matches(r#"data-testid="map-area""#).count(), 3);
        assert!(s.contains(">Stage</text>") && s.contains(">no place set</text>"));
        assert!(s.contains(&format!(r#"viewBox="0 0 {:.0} "#, p.width)));
    }

    #[test]
    fn a_line_carries_its_latency_without_trailing_zeros() {
        assert_eq!(ms_short(5000), "5 ms");
        assert_eq!(ms_short(250), "0.25 ms");
        assert_eq!(ms_short(8360), "8.36 ms");
        assert_eq!(ms_short(10_000), "10 ms");
        assert_eq!(ms_short(1500), "1.5 ms");
    }

    #[test]
    fn devices_that_feed_each_other_sit_in_the_two_middle_columns_not_one() {
        // #142: a real network has a dozen devices that send and receive,
        // feeding each other; in one column every such line was an arc
        // over the boxes. Two mixers feeding each other and a recorder
        // that both feed (and that sends on) go across, not inside.
        let d = Devices {
            devices: vec![
                dev("src", 2, &[]),
                dev("mix-a", 2, &[Some("src"), Some("mix-b")]),
                dev("mix-b", 2, &[Some("mix-a"), Some("src")]),
                dev("rec", 2, &[Some("mix-a")]),
                dev("amp", 0, &[Some("rec"), Some("mix-b")]),
            ],
            ..Devices::default()
        };
        let g = build(&d, None, Span::H1);
        let p = layout(&g);
        let col = |name: &str| p.cols[g.nodes.iter().position(|n| n.name == name).unwrap()];
        assert_eq!((col("src"), col("amp")), (0, 3));
        assert_ne!(col("mix-a"), col("mix-b"));
        assert_ne!(col("mix-a"), col("rec"));
        // No connection between two devices in one column: no arc.
        let same = g
            .edges
            .iter()
            .filter(|e| e.from != e.to && p.cols[e.from] == p.cols[e.to])
            .count();
        assert_eq!(same, 0);
        // Its boxes and labels keep off one another.
        for (i, a) in p.nodes.iter().enumerate() {
            for b in &p.nodes[i + 1..] {
                assert!(!a.hits(b), "boxes overlap");
            }
        }
    }

    #[test]
    fn a_device_subscribed_to_itself_is_a_loop_on_its_box() {
        let d = Devices {
            devices: vec![dev("solo", 2, &[Some("solo")])],
            ..Devices::default()
        };
        let g = build(&d, None, Span::H1);
        assert_eq!((g.nodes.len(), g.edges.len()), (1, 1));
        let p = layout(&g);
        assert_eq!(p.cols, [1]);
        assert!(p.edges[0].d.starts_with('M') && p.edges[0].d.contains('C'));
        assert!(!p.nodes[0].hits(&p.edges[0].label));
    }

    #[test]
    fn the_drawing_names_every_box_and_line_and_escapes_names() {
        let mut d = net();
        d.devices[0].name = "amp<1>&\"x\"".into();
        d.devices[4].clock = Some(Clock {
            grandmaster: true,
            ..Clock::default()
        });
        d.devices[2].clock = Some(Clock {
            sync: "none".into(),
            ..Clock::default()
        });
        let l = links();
        let g = build(&d, Some(&l), Span::H1);
        let p = layout(&g);
        let chosen = Sel::Link {
            from: "player".into(),
            to: "bridge".into(),
        };
        let s = svg(&g, &p, Some(&chosen));
        assert!(s.starts_with(r#"<svg class="map has-sel""#));
        assert_eq!(s.matches(r#"data-testid="map-node""#).count(), 9);
        assert_eq!(s.matches(r#"data-testid="map-edge""#).count(), 9);
        assert!(s.contains(r#"data-node="amp&lt;1&gt;&amp;&quot;x&quot;""#));
        assert!(!s.contains("amp<1>"));
        assert!(s.contains(r##"class="edge bad sel" data-testid="map-edge" data-edge="#map/link/player/bridge" data-from="player" data-to="bridge" data-state="bad""##));
        assert!(s.contains(r#"<tspan class="lost"> · 67 004 late</tspan>"#));
        assert!(s.contains(">PTP GM</text>"));
        assert!(s.contains(r#"class="badge bad""#));
        assert_eq!(s.matches(" rel\"").count(), 2);
        assert!(s.contains(r#"class="node asio""#));
        assert!(s.contains(r#"class="node missing""#));
        // A chosen device: its box, its connections and their other ends.
        let dv = svg(
            &g,
            &p,
            Some(&Sel::Device {
                name: "BRIDGE".into(),
            }),
        );
        assert!(dv.starts_with(r#"<svg class="map has-sel has-device""#));
        assert!(dv.contains(r#"class="node sel" data-testid="map-node" data-node="bridge""#));
        // player in, stream out, and its two ways to the ASIO side.
        assert_eq!(dv.matches(" of\"").count() + dv.matches(" of ").count(), 4);
        assert_eq!(
            dv.matches(r#"class="node rel""#).count()
                + dv.matches(r#"class="node asio rel""#).count(),
            3
        );
        assert!(has(
            &g,
            &Sel::Device {
                name: "Bridge".into()
            }
        ));
        assert!(!has(
            &g,
            &Sel::Device {
                name: "SoundGrid".into()
            }
        ));
        assert!(!has(
            &g,
            &Sel::Device {
                name: "gone".into()
            }
        ));
        assert!(has(&g, &chosen));
        let [ins, outs, inside] = edges_of(&g, node_of(&g, "bridge").unwrap());
        assert_eq!((ins.len(), outs.len(), inside.len()), (1, 1, 2));
        // Nothing chosen: nothing stands out.
        assert!(svg(&g, &p, None).starts_with(r#"<svg class="map" "#));
        // A small network shows every line's number.
        assert!(!s.contains(" quiet"));
        assert_eq!(num(1_234_567), "1 234 567");
        assert_eq!(num(12), "12");
    }
}
