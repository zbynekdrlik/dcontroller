//! The latency history's numbers for the page: the ranges offered, each
//! flow's colour and label, the chart's data for `charts.js`, the overview
//! sparklines and a receiver's health. Pure, so it is tested on the host.

use dcontroller_api::{FlowHistory, History, ReceiverSummary};
use serde::Serialize;

/// The ranges offered: label and seconds.
pub const RANGES: [(&str, i64); 7] = [
    ("5 min", 300),
    ("1 h", 3600),
    ("6 h", 6 * 3600),
    ("24 h", 24 * 3600),
    ("7 d", 7 * 24 * 3600),
    ("30 d", 30 * 24 * 3600),
    ("1 y", 365 * 24 * 3600),
];

/// The page's history reads: numbered, so an answer that comes after a
/// newer read (a slow 1 y read after a 5 min click) is dropped, and busy
/// while the newest runs, so a live poll never supersedes a slow read.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HistReads {
    last: u64,
    /// The newest read a choice started (a range, a zoom, a device): an
    /// answer older than it is stale; live polls of the same range are not.
    chosen: u64,
    /// When the newest read started (ms), while it runs.
    running: Option<f64>,
}

/// How long a read holds the live poll before it is taken as lost, ms.
pub const READ_HOLD_MS: f64 = 30_000.0;

impl HistReads {
    /// A new read a choice started, at `now_ms`: its number.
    pub fn start_at(&mut self, now_ms: f64) -> u64 {
        let n = self.poll_at(now_ms);
        self.chosen = n;
        n
    }

    /// A new live poll of the same range, at `now_ms`: its number.
    pub fn poll_at(&mut self, now_ms: f64) -> u64 {
        self.last += 1;
        self.running = Some(now_ms);
        self.last
    }

    /// A new read's number, at time zero (tests).
    #[cfg(test)]
    pub fn start(&mut self) -> u64 {
        self.start_at(0.0)
    }

    /// Whether the newest read is still running, however long (tests).
    #[cfg(test)]
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Read `n` answered: whether it may show (no choice was made since).
    pub fn done(&mut self, n: u64) -> bool {
        if n == self.last {
            self.running = None;
        }
        n >= self.chosen
    }

    /// Whether the newest read still runs at `now_ms` and is not taken as
    /// lost (a read that never answers stops holding the live poll).
    pub fn busy_at(&self, now_ms: f64) -> bool {
        self.running.is_some_and(|t| now_ms - t < READ_HOLD_MS)
    }
}

/// Dante device names compare case aside.
pub fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The range a page opens with.
pub const DEFAULT_RANGE: i64 = 3600;

/// Distinct line colours for flows, readable on the dark background.
pub const PALETTE: [&str; 8] = [
    "#5b9dff", "#34d399", "#c084fc", "#fbbf24", "#22d3ee", "#f472b6", "#a3e635", "#fb923c",
];

/// The colour of the `i`-th flow.
pub fn color(i: usize) -> &'static str {
    PALETTE[i % PALETTE.len()]
}

/// What a flow comes from: its source devices, or `?` when not known.
fn source_of(f: &FlowHistory) -> String {
    if f.sources.is_empty() {
        "?".to_owned()
    } else {
        f.sources.join(", ")
    }
}

/// A receiver's flows from one source device, read as one: device to
/// device. The late packets are the damage (packets that came after their
/// flow's latency and were not played); the peak against each flow's own
/// latency is only the margin.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceHistory {
    pub source: String,
    pub slots: Vec<u16>,
    pub channels: Vec<String>,
    /// The flows' latencies, distinct, smallest first.
    pub latencies: Vec<u32>,
    /// Per bucket: the worst peak as a share of its own flow's latency, %
    /// (`None`: nothing recorded, or no latency known).
    pub share: Vec<Option<f64>>,
    /// Per bucket: the largest peak, µs.
    pub max_us: Vec<Option<u32>>,
    /// Per bucket: the typical second, the mean of the per-second peaks,
    /// as a share of its own flow's latency, % (the flow furthest along).
    pub typical: Vec<Option<f64>>,
    /// Per bucket: the largest mean of the per-second peaks, µs.
    pub typical_us: Vec<Option<u32>>,
    /// Per bucket: the late packets of all its flows.
    pub late: Vec<u64>,
    pub late_total: u64,
    /// Over the range: the peak closest to (or furthest past) its flow's
    /// latency, and that latency.
    pub peak_us: u32,
    pub peak_latency_us: u32,
    /// Stored values past their latency, over all its flows.
    pub over: u64,
    /// The flows' distributions added up (the bins are shares of each
    /// flow's own latency, so they add).
    pub histogram: Vec<u64>,
}

/// `h`'s flows grouped by source device, by name.
pub fn by_source(h: &History) -> Vec<SourceHistory> {
    let n = h.t.len();
    let mut out: Vec<SourceHistory> = Vec::new();
    for f in &h.flows {
        let name = source_of(f);
        let (i, first) = match out.iter().position(|s| s.source == name) {
            Some(i) => (i, false),
            None => {
                out.push(SourceHistory {
                    source: name,
                    share: vec![None; n],
                    max_us: vec![None; n],
                    typical: vec![None; n],
                    typical_us: vec![None; n],
                    late: vec![0; n],
                    ..SourceHistory::default()
                });
                (out.len() - 1, true)
            }
        };
        let s = &mut out[i];
        if !s.slots.contains(&f.slot) {
            s.slots.push(f.slot);
        }
        for c in &f.channels {
            if !s.channels.contains(c) {
                s.channels.push(c.clone());
            }
        }
        if f.latency_us > 0 && !s.latencies.contains(&f.latency_us) {
            s.latencies.push(f.latency_us);
            s.latencies.sort_unstable();
        }
        for k in 0..n {
            if let Some(us) = f.max_us.get(k).copied().flatten() {
                s.max_us[k] = Some(s.max_us[k].map_or(us, |m| m.max(us)));
                if f.latency_us > 0 {
                    let p = f64::from(us) * 100.0 / f64::from(f.latency_us);
                    s.share[k] = Some(s.share[k].map_or(p, |m| m.max(p)));
                }
            }
            if let Some(us) = f.mean_us.get(k).copied().flatten() {
                s.typical_us[k] = Some(s.typical_us[k].map_or(us, |m| m.max(us)));
                if f.latency_us > 0 {
                    let p = f64::from(us) * 100.0 / f64::from(f.latency_us);
                    s.typical[k] = Some(s.typical[k].map_or(p, |m| m.max(p)));
                }
            }
            s.late[k] += f.late.get(k).copied().unwrap_or(0);
        }
        s.late_total += f.late_total;
        s.over += f.over;
        let worse = |a: (u32, u32), b: (u32, u32)| {
            // a/la > b/lb, a flow of unknown latency by its peak alone.
            match (a.1, b.1) {
                (0, 0) => a.0 > b.0,
                (0, _) => false,
                (_, 0) => true,
                _ => u64::from(a.0) * u64::from(b.1) > u64::from(b.0) * u64::from(a.1),
            }
        };
        if first || worse((f.peak_us, f.latency_us), (s.peak_us, s.peak_latency_us)) {
            s.peak_us = f.peak_us;
            s.peak_latency_us = f.latency_us;
        }
        if s.histogram.len() < f.histogram.len() {
            s.histogram.resize(f.histogram.len(), 0);
        }
        for (a, b) in s.histogram.iter_mut().zip(&f.histogram) {
            *a += b;
        }
    }
    out.sort_by_key(|s| s.source.to_lowercase());
    out
}

/// µs as ms with two decimals.
pub fn ms(us: u32) -> String {
    format!("{:.2} ms", f64::from(us) / 1000.0)
}

/// How many stored values passed the latency, in the tier's unit.
pub fn over_text(over: u64, tier_s: i64) -> String {
    let unit = match tier_s {
        1 => "s",
        60 => "min",
        _ => "h",
    };
    format!("{over} {unit}")
}

/// How the chart's y axis reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    /// Each flow's peak as a share of its own latency (100 % = at it): flows
    /// of different latencies on one axis.
    Share,
    /// The peaks in ms, with a line per distinct latency.
    Ms,
}

#[derive(Serialize)]
struct ChartSource {
    label: String,
    color: &'static str,
    /// The margin: the typical second per bucket (the mean of its
    /// per-second peaks) in the chart's unit. The worst second of an hour
    /// would paint the whole hour; it stays in the table.
    values: Vec<Option<f64>>,
    /// The damage: late packets per bucket.
    late: Vec<u64>,
}

#[derive(Serialize)]
struct Threshold {
    label: String,
    color: &'static str,
    value: f64,
}

#[derive(Serialize)]
struct Chart<'a> {
    t: &'a [i64],
    unit: &'static str,
    sources: Vec<ChartSource>,
    thresholds: Vec<Threshold>,
    /// Whether any source has late packets in the range (the late chart
    /// shows only then).
    late: bool,
    /// The margin's y axis reaches at least the (smallest) latency.
    floor: f64,
    height: u32,
}

/// The chart's data for `dcChart.latency`: per source device its late
/// packets (the damage) and its worst peak per bucket (the margin), and the
/// latency line(s). `Share` leaves out the margin of a source whose
/// latency is not known.
pub fn chart_json(h: &History, height: u32, scale: Scale) -> String {
    let groups = by_source(h);
    let sources: Vec<ChartSource> = groups
        .iter()
        .enumerate()
        .map(|(i, s)| ChartSource {
            label: s.source.clone(),
            color: color(i),
            values: match scale {
                Scale::Ms => s
                    .typical_us
                    .iter()
                    .map(|v| v.map(|us| f64::from(us) / 1000.0))
                    .collect(),
                Scale::Share => s.typical.clone(),
            },
            late: s.late.clone(),
        })
        .collect();
    let mut lats: Vec<u32> = groups.iter().flat_map(|s| s.latencies.clone()).collect();
    lats.sort_unstable();
    lats.dedup();
    let thresholds: Vec<Threshold> = match scale {
        Scale::Ms => lats
            .iter()
            .map(|&us| Threshold {
                label: format!("latency {}", ms(us)),
                color: "#f87171",
                value: f64::from(us) / 1000.0,
            })
            .collect(),
        Scale::Share if lats.is_empty() => Vec::new(),
        Scale::Share => vec![Threshold {
            label: "latency".into(),
            color: "#f87171",
            value: 100.0,
        }],
    };
    let chart = Chart {
        t: &h.t,
        unit: match scale {
            Scale::Ms => "ms",
            Scale::Share => "%",
        },
        floor: thresholds.first().map_or(0.0, |t| t.value * 1.05),
        late: groups.iter().any(|s| s.late.iter().any(|&l| l > 0)),
        sources,
        thresholds,
        height,
    };
    serde_json::to_string(&chart).unwrap_or_else(|_| "{}".into())
}

/// An SVG path through `values` (per mille of the latency) in a `w`×`h`
/// box, 0 at the bottom and at least `top` at the top; a gap breaks it.
pub fn spark_path(values: &[Option<u32>], w: f64, h: f64, top: u32) -> String {
    let top = values
        .iter()
        .flatten()
        .copied()
        .max()
        .unwrap_or(0)
        .max(top)
        .max(1);
    let n = values.len().max(2) - 1;
    let mut out = String::new();
    let mut pen = false;
    for (i, v) in values.iter().enumerate() {
        match v {
            Some(v) => {
                let x = w * i as f64 / n as f64;
                let y = h - h * f64::from(*v) / f64::from(top);
                out.push_str(&format!("{}{x:.1},{y:.1} ", if pen { "L" } else { "M" }));
                pen = true;
            }
            None => pen = false,
        }
    }
    out.trim_end().to_owned()
}

/// A receiver's health from its summary: CSS class and text. Only late
/// packets (the receiver's own count of packets that came after their
/// flow's latency, so were not played) count, by the seconds they hit: in
/// the last 5 minutes it is bad (happening now), earlier in the hour a
/// warning; with none it is fine, whatever its peak.
pub fn health(r: &ReceiverSummary) -> (&'static str, String) {
    if r.late_s_5m > 0 {
        return ("bad", format!("late now: {} s of 5 min", r.late_s_5m));
    }
    if r.late_s_1h > 0 {
        return ("warn", format!("late in {} s of 1 h", r.late_s_1h));
    }
    ("ok", "no late packets".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(slot: u16, lat: u32, max: Vec<Option<u32>>, late: Vec<u64>) -> FlowHistory {
        from("desk", slot, lat, max, late)
    }

    fn from(src: &str, slot: u16, lat: u32, max: Vec<Option<u32>>, late: Vec<u64>) -> FlowHistory {
        FlowHistory {
            slot,
            latency_us: lat,
            sources: vec![src.into()],
            channels: vec![format!("ch {slot}")],
            peak_us: max.iter().flatten().copied().max().unwrap_or(0),
            late_total: late.iter().sum(),
            histogram: vec![1; 11],
            mean_us: max.iter().map(|v| v.map(|us| us / 2)).collect(),
            max_us: max,
            late,
            ..FlowHistory::default()
        }
    }

    #[test]
    fn flows_from_one_source_device_read_as_one_device_to_device() {
        let h = History {
            t: vec![10, 20, 30],
            flows: vec![
                from(
                    "mixer",
                    1,
                    1000,
                    vec![Some(100), None, Some(300)],
                    vec![0, 0, 2],
                ),
                from(
                    "desk",
                    2,
                    250,
                    vec![Some(50), Some(200), None],
                    vec![0, 1, 0],
                ),
                from(
                    "mixer",
                    3,
                    250,
                    vec![Some(200), Some(60), None],
                    vec![0, 4, 0],
                ),
                // The same slot again, after its flow changed: still one source.
                from("mixer", 1, 1000, vec![None, None, Some(400)], vec![0, 0, 1]),
            ],
            ..History::default()
        };
        let g = by_source(&h);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].source, "desk");
        let m = &g[1];
        assert_eq!(m.source, "mixer");
        assert_eq!(m.slots, vec![1, 3]);
        assert_eq!(m.channels, vec!["ch 1", "ch 3"]);
        assert_eq!(m.latencies, vec![250, 1000]);
        // Per bucket, the worst share of its own flow's latency.
        assert_eq!(m.share, vec![Some(80.0), Some(24.0), Some(40.0)]);
        assert_eq!(m.max_us, vec![Some(200), Some(60), Some(400)]);
        // The typical second: the mean of the peaks, here half of each.
        assert_eq!(m.typical, vec![Some(40.0), Some(12.0), Some(20.0)]);
        assert_eq!(m.typical_us, vec![Some(100), Some(30), Some(200)]);
        assert_eq!(m.late, vec![0, 4, 3]);
        assert_eq!(m.late_total, 7);
        // The worst peak is the one closest to its latency (200 of 250), not the largest.
        assert_eq!((m.peak_us, m.peak_latency_us), (200, 250));
        assert_eq!(m.histogram, vec![3; 11]);
    }

    #[test]
    fn the_chart_has_a_line_and_late_bars_per_source_device_and_a_threshold_per_latency() {
        let h = History {
            t: vec![10, 20, 30],
            flows: vec![
                flow(1, 1000, vec![Some(100), None, Some(300)], vec![0, 0, 2]),
                flow(2, 1000, vec![Some(50), Some(60), Some(70)], vec![0, 1, 0]),
                from("tascam", 3, 250, vec![None, None, Some(10)], vec![0, 0, 0]),
            ],
            ..History::default()
        };
        let v: serde_json::Value = serde_json::from_str(&chart_json(&h, 300, Scale::Ms)).unwrap();
        assert_eq!(v["t"], serde_json::json!([10, 20, 30]));
        assert_eq!(v["unit"], "ms");
        assert_eq!(v["sources"].as_array().unwrap().len(), 2);
        assert_eq!(v["sources"][0]["label"], "desk");
        assert_eq!(v["sources"][0]["color"], PALETTE[0]);
        assert_eq!(
            v["sources"][0]["values"],
            // The line is the typical second (the mean), not the worst one.
            serde_json::json!([0.05, 0.03, 0.15])
        );
        assert_eq!(v["sources"][0]["late"], serde_json::json!([0, 1, 2]));
        assert_eq!(v["sources"][1]["label"], "tascam");
        assert_eq!(v["thresholds"].as_array().unwrap().len(), 2);
        assert_eq!(v["thresholds"][0]["value"], 0.25);
        assert_eq!(v["thresholds"][1]["label"], "latency 1.00 ms");
        assert_eq!(v["late"], true);
        assert_eq!(v["height"], 300);
        // As a share of each flow's own latency: one 100 % line for all.
        let p: serde_json::Value =
            serde_json::from_str(&chart_json(&h, 300, Scale::Share)).unwrap();
        assert_eq!(p["unit"], "%");
        assert_eq!(
            p["sources"][0]["values"],
            serde_json::json!([5.0, 3.0, 15.0])
        );
        assert_eq!(
            p["sources"][1]["values"],
            serde_json::json!([null, null, 2.0])
        );
        assert_eq!(
            p["thresholds"],
            serde_json::json!([{"label": "latency", "color": "#f87171", "value": 100.0}])
        );
        assert_eq!(p["floor"], 105.0);
        // No late packets: no late chart.
        let quiet = History {
            t: vec![1],
            flows: vec![flow(1, 1000, vec![Some(1)], vec![0])],
            ..History::default()
        };
        let q: serde_json::Value =
            serde_json::from_str(&chart_json(&quiet, 200, Scale::Share)).unwrap();
        assert_eq!(q["late"], false);
    }

    #[test]
    fn a_read_that_never_answers_stops_holding_the_live_poll() {
        let mut r = HistReads::default();
        let _hung = r.start_at(1_000.0);
        assert!(r.busy_at(1_000.0 + 29_000.0));
        assert!(!r.busy_at(1_000.0 + 31_000.0));
    }

    #[test]
    fn a_slow_read_still_shows_when_live_polls_follow_it() {
        // Every read takes over 30 s: the polls after it ask the same range,
        // so the slow answer is still the page's to show.
        let mut r = HistReads::default();
        let slow = r.start_at(0.0);
        assert!(!r.busy_at(31_000.0));
        let poll = r.poll_at(31_000.0);
        assert!(r.done(slow));
        assert!(r.done(poll));
        // A choice made since (a range, a zoom) still drops an older answer.
        let year = r.start_at(40_000.0);
        let five = r.start_at(41_000.0);
        assert!(!r.done(year));
        assert!(r.done(five));
    }

    #[test]
    fn device_names_compare_case_aside() {
        assert!(same_name("Stage-1", "stage-1"));
        assert!(!same_name("stage-1", "stage-2"));
    }

    #[test]
    fn only_the_latest_read_may_show_and_a_live_poll_waits_for_it() {
        // A slow 1 y read answered after a newer 5 min read must not replace it.
        let mut r = HistReads::default();
        let year = r.start();
        let five = r.start();
        assert!(!r.done(year));
        // A live poll does not start while a read runs: a read slower than
        // the poll still gets shown.
        assert!(r.busy());
        assert!(r.done(five));
        assert!(!r.busy());
    }

    #[test]
    fn sparklines_break_at_gaps_and_scale_to_the_latency_at_least() {
        assert_eq!(
            spark_path(&[Some(0), Some(500), Some(1000)], 100.0, 10.0, 1000),
            "M0.0,10.0 L50.0,5.0 L100.0,0.0"
        );
        assert_eq!(
            spark_path(&[Some(500), None, Some(500)], 100.0, 10.0, 1000),
            "M0.0,5.0 M100.0,5.0"
        );
        // A peak past the latency raises the top.
        assert_eq!(
            spark_path(&[Some(2000), Some(1000)], 10.0, 10.0, 1000),
            "M0.0,0.0 L10.0,5.0"
        );
        assert_eq!(spark_path(&[None, None], 10.0, 10.0, 1000), "");
    }

    #[test]
    fn health_reads_the_seconds_with_late_packets_never_the_peak() {
        let r = |peak_us, latency_us, late_s_1h| ReceiverSummary {
            peak_us,
            latency_us,
            late_s_1h,
            ..ReceiverSummary::default()
        };
        // Late seconds earlier in the hour: a warning; in the last 5 min: bad.
        assert_eq!(
            health(&r(100, 1000, 3)),
            ("warn", "late in 3 s of 1 h".to_owned())
        );
        let now = ReceiverSummary {
            late_s_1h: 3,
            late_s_5m: 1,
            ..ReceiverSummary::default()
        };
        assert_eq!(health(&now), ("bad", "late now: 1 s of 5 min".to_owned()));
        // No late packets: fine, whatever the peak (even past the latency).
        assert_eq!(
            health(&r(900, 1000, 0)),
            ("ok", "no late packets".to_owned())
        );
        assert_eq!(
            health(&r(7000, 1000, 0)),
            ("ok", "no late packets".to_owned())
        );
        assert_eq!(over_text(3, 1), "3 s");
        assert_eq!(over_text(3, 60), "3 min");
        assert_eq!(over_text(3, 3600), "3 h");
        assert_eq!(source_of(&FlowHistory::default()), "?");
        assert_eq!(color(9), PALETTE[1]);
    }
}
