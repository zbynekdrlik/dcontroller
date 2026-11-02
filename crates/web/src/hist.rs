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

/// Numbers the page's history reads, so an answer that comes after a
/// newer request (a slow 1 y read after a 5 min click) is dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Requests {
    last: u64,
}

impl Requests {
    /// A new request's number.
    pub fn next(&mut self) -> u64 {
        self.last += 1;
        self.last
    }

    /// Whether `n` is still the newest request.
    pub fn is_latest(&self, n: u64) -> bool {
        n == self.last
    }
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

/// A flow's label: its slot and where it comes from.
pub fn label(f: &FlowHistory) -> String {
    if f.sources.is_empty() {
        format!("Flow {}", f.slot)
    } else {
        format!("Flow {} · {}", f.slot, f.sources.join(", "))
    }
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
struct ChartFlow {
    label: String,
    color: &'static str,
    values: Vec<Option<f64>>,
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
    flows: Vec<ChartFlow>,
    thresholds: Vec<Threshold>,
    /// Late packets per bucket over all flows; `None` when there are none.
    late: Option<Vec<u64>>,
    /// The y axis reaches at least the (smallest) latency, so the margin shows.
    floor: f64,
    height: u32,
}

/// The chart's data for `dcChart.latency`: a line per flow (its largest
/// peak per bucket), the latency line(s), and late packets as bars when
/// there are any. `Share` leaves out a flow whose latency is not known.
pub fn chart_json(h: &History, height: u32, scale: Scale) -> String {
    let flows = h
        .flows
        .iter()
        .enumerate()
        .filter(|(_, f)| scale == Scale::Ms || f.latency_us > 0)
        .map(|(i, f)| ChartFlow {
            label: label(f),
            color: color(i),
            values: f
                .max_us
                .iter()
                .map(|v| {
                    v.map(|us| match scale {
                        Scale::Ms => f64::from(us) / 1000.0,
                        Scale::Share => f64::from(us) * 100.0 / f64::from(f.latency_us),
                    })
                })
                .collect(),
        })
        .collect();
    let mut lats: Vec<u32> = h
        .flows
        .iter()
        .map(|f| f.latency_us)
        .filter(|&l| l > 0)
        .collect();
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
    let late: Vec<u64> = (0..h.t.len())
        .map(|i| {
            h.flows
                .iter()
                .map(|f| f.late.get(i).copied().unwrap_or(0))
                .sum()
        })
        .collect();
    let chart = Chart {
        t: &h.t,
        unit: match scale {
            Scale::Ms => "ms",
            Scale::Share => "%",
        },
        floor: thresholds.first().map_or(0.0, |t| t.value * 1.05),
        flows,
        thresholds,
        late: late.iter().any(|&l| l > 0).then_some(late),
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

/// A receiver's health from its summary: CSS class and text. Late packets
/// in the last hour are bad; a peak past 80 % of the latency is a warning.
pub fn health(r: &ReceiverSummary) -> (&'static str, String) {
    if r.late_1h > 0 {
        return ("bad", format!("{} late in 1 h", r.late_1h));
    }
    if r.latency_us > 0 && u64::from(r.peak_us) * 10 > u64::from(r.latency_us) * 8 {
        return (
            "warn",
            format!("peak {} of {}", ms(r.peak_us), ms(r.latency_us)),
        );
    }
    if r.latency_us > 0 {
        let share = u64::from(r.peak_us) * 100 / u64::from(r.latency_us);
        return ("ok", format!("peak {share} % of latency"));
    }
    ("ok", format!("peak {}", ms(r.peak_us)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(slot: u16, lat: u32, max: Vec<Option<u32>>, late: Vec<u64>) -> FlowHistory {
        FlowHistory {
            slot,
            latency_us: lat,
            sources: vec!["desk".into()],
            max_us: max,
            late,
            ..FlowHistory::default()
        }
    }

    #[test]
    fn the_chart_has_a_line_per_flow_a_threshold_per_latency_and_late_bars() {
        let h = History {
            t: vec![10, 20, 30],
            flows: vec![
                flow(1, 1000, vec![Some(100), None, Some(300)], vec![0, 0, 2]),
                flow(2, 1000, vec![Some(50), Some(60), Some(70)], vec![0, 1, 0]),
                flow(3, 250, vec![None, None, Some(10)], vec![0, 0, 0]),
            ],
            ..History::default()
        };
        let v: serde_json::Value = serde_json::from_str(&chart_json(&h, 300, Scale::Ms)).unwrap();
        assert_eq!(v["t"], serde_json::json!([10, 20, 30]));
        assert_eq!(v["unit"], "ms");
        assert_eq!(v["flows"].as_array().unwrap().len(), 3);
        assert_eq!(v["flows"][0]["label"], "Flow 1 · desk");
        assert_eq!(v["flows"][0]["color"], PALETTE[0]);
        assert_eq!(v["flows"][0]["values"], serde_json::json!([0.1, null, 0.3]));
        assert_eq!(v["thresholds"].as_array().unwrap().len(), 2);
        assert_eq!(v["thresholds"][0]["value"], 0.25);
        assert_eq!(v["thresholds"][1]["label"], "latency 1.00 ms");
        assert_eq!(v["late"], serde_json::json!([0, 1, 2]));
        assert_eq!(v["height"], 300);
        // As a share of each flow's own latency: one 100 % line for all.
        let p: serde_json::Value =
            serde_json::from_str(&chart_json(&h, 300, Scale::Share)).unwrap();
        assert_eq!(p["unit"], "%");
        assert_eq!(
            p["flows"][0]["values"],
            serde_json::json!([10.0, null, 30.0])
        );
        assert_eq!(
            p["flows"][2]["values"],
            serde_json::json!([null, null, 4.0])
        );
        assert_eq!(
            p["thresholds"],
            serde_json::json!([{"label": "latency", "color": "#f87171", "value": 100.0}])
        );
        assert_eq!(p["floor"], 105.0);
        // No late packets: no bars.
        let quiet = History {
            t: vec![1],
            flows: vec![flow(1, 1000, vec![Some(1)], vec![0])],
            ..History::default()
        };
        let q: serde_json::Value =
            serde_json::from_str(&chart_json(&quiet, 200, Scale::Share)).unwrap();
        assert!(q["late"].is_null());
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
    fn health_reads_late_packets_and_the_margin() {
        let r = |peak_us, latency_us, late_1h| ReceiverSummary {
            peak_us,
            latency_us,
            late_1h,
            ..ReceiverSummary::default()
        };
        assert_eq!(
            health(&r(100, 1000, 3)),
            ("bad", "3 late in 1 h".to_owned())
        );
        assert_eq!(
            health(&r(900, 1000, 0)),
            ("warn", "peak 0.90 ms of 1.00 ms".to_owned())
        );
        assert_eq!(
            health(&r(110, 250, 0)),
            ("ok", "peak 44 % of latency".to_owned())
        );
        assert_eq!(health(&r(110, 0, 0)), ("ok", "peak 0.11 ms".to_owned()));
        assert_eq!(over_text(3, 1), "3 s");
        assert_eq!(over_text(3, 60), "3 min");
        assert_eq!(over_text(3, 3600), "3 h");
        assert_eq!(
            label(&FlowHistory {
                slot: 2,
                ..FlowHistory::default()
            }),
            "Flow 2"
        );
        assert_eq!(color(9), PALETTE[1]);
    }
}
