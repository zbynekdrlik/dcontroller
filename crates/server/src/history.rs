//! The latency history: every receiver's per-second peak lateness and late
//! packets per RX flow, kept in SQLite in three tiers (seconds for a day,
//! minutes for a month, hours for a year) and read back bucketed for any
//! range (`GET /api/history/{device}`). The recorder (`recorder.rs`) fills
//! it; nothing here talks to a device.

use std::path::Path;
use std::sync::Mutex;

use dcontroller_api as api;
use rusqlite::{Connection, OptionalExtension, params};
use tracing::info;

/// How long each tier is kept, seconds.
pub const KEEP_SECONDS: i64 = 24 * 3600;
pub const KEEP_MINUTES: i64 = 30 * 24 * 3600;
pub const KEEP_HOURS: i64 = 365 * 24 * 3600;

/// The schema, one step per version (`PRAGMA user_version`). A step that
/// shipped is never edited: a change is a new step.
const MIGRATIONS: &[&str] = &[
    // 1: the first schema.
    "CREATE TABLE device (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
     CREATE TABLE second (device INTEGER NOT NULL, slot INTEGER NOT NULL, t INTEGER NOT NULL,
         peak INTEGER NOT NULL, late INTEGER NOT NULL, PRIMARY KEY (device, t, slot)) WITHOUT ROWID;
     CREATE TABLE minute (device INTEGER NOT NULL, slot INTEGER NOT NULL, t INTEGER NOT NULL,
         peak INTEGER NOT NULL, sum INTEGER NOT NULL, n INTEGER NOT NULL, late INTEGER NOT NULL,
         PRIMARY KEY (device, t, slot)) WITHOUT ROWID;
     CREATE TABLE hour (device INTEGER NOT NULL, slot INTEGER NOT NULL, t INTEGER NOT NULL,
         peak INTEGER NOT NULL, sum INTEGER NOT NULL, n INTEGER NOT NULL, late INTEGER NOT NULL,
         PRIMARY KEY (device, t, slot)) WITHOUT ROWID;
     CREATE TABLE flow (device INTEGER NOT NULL, slot INTEGER NOT NULL, since INTEGER NOT NULL,
         latency_us INTEGER NOT NULL, rate INTEGER NOT NULL, channels TEXT NOT NULL,
         sources TEXT NOT NULL, PRIMARY KEY (device, slot, since)) WITHOUT ROWID;",
];

/// One second of one flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub slot: u16,
    /// Unix seconds.
    pub t: i64,
    pub peak_us: u32,
    pub late: u32,
}

/// A flow's description as the recorder read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlowInfo {
    pub latency_us: u32,
    pub rate: u32,
    pub channels: Vec<String>,
    pub sources: Vec<String>,
}

/// The tier a range reads from and the bucket size: seconds up to 2 h,
/// minutes up to 7 days, hours beyond; at most [`api::HISTORY_POINTS`]
/// buckets, each a whole number of the tier's unit.
pub fn plan(span_s: i64) -> (Tier, i64) {
    let span = span_s.max(1);
    let tier = if span <= 2 * 3600 {
        Tier::Second
    } else if span <= 7 * 24 * 3600 {
        Tier::Minute
    } else {
        Tier::Hour
    };
    let unit = tier.unit_s();
    let per = (span + api::HISTORY_POINTS - 1) / api::HISTORY_POINTS;
    let step = ((per + unit - 1) / unit).max(1) * unit;
    (tier, step)
}

/// A stored resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Second,
    Minute,
    Hour,
}

impl Tier {
    pub fn unit_s(self) -> i64 {
        match self {
            Tier::Second => 1,
            Tier::Minute => 60,
            Tier::Hour => 3600,
        }
    }

    /// The rows of device `?1` at this resolution as `(slot, t, peak, sum,
    /// n, late)`: the tier's own rows, then what the finer tiers hold past
    /// its last rolled-up row, aggregated on the fly (the end of any range,
    /// and everything before the first rollup).
    fn source(self) -> String {
        const SECONDS: &str =
            "SELECT slot, t, peak, peak AS sum, 1 AS n, late FROM second WHERE device = ?1";
        let minutes = String::from(
            "SELECT slot, t, peak, sum, n, late FROM minute WHERE device = ?1 UNION ALL
             SELECT slot, t - t % 60, MAX(peak), SUM(peak), COUNT(*), SUM(late) FROM second
             WHERE device = ?1 AND t >= (SELECT COALESCE(MAX(t) + 60, 0) FROM minute WHERE device = ?1)
             GROUP BY slot, t - t % 60"
        );
        match self {
            Tier::Second => SECONDS.to_owned(),
            Tier::Minute => minutes,
            Tier::Hour => format!(
                "SELECT slot, t, peak, sum, n, late FROM hour WHERE device = ?1 UNION ALL
                 SELECT slot, t - t % 3600, MAX(peak), SUM(sum), SUM(n), SUM(late) FROM ({minutes})
                 WHERE t >= (SELECT COALESCE(MAX(t) + 3600, 0) FROM hour WHERE device = ?1)
                 GROUP BY slot, t - t % 3600"
            ),
        }
    }
}

/// The store: one SQLite file, one connection, one writer at a time.
pub struct Store {
    db: Mutex<Connection>,
}

impl Store {
    /// Opens (or creates) the database at `path` and brings its schema up.
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        let db = Connection::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
        db.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("WAL: {e}"))?;
        db.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| format!("synchronous: {e}"))?;
        Self::with(db)
    }

    /// An in-memory store (tests).
    pub fn memory() -> Result<Self, String> {
        Self::with(Connection::open_in_memory().map_err(|e| e.to_string())?)
    }

    fn with(db: Connection) -> Result<Self, String> {
        let at: i64 = db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| format!("schema version: {e}"))?;
        for (i, step) in MIGRATIONS
            .iter()
            .enumerate()
            .skip(usize::try_from(at).unwrap_or(0))
        {
            db.execute_batch(&format!(
                "BEGIN; {step}; PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))
            .map_err(|e| format!("schema step {}: {e}", i + 1))?;
            info!(version = i + 1, "history: schema step applied");
        }
        Ok(Store { db: Mutex::new(db) })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn device_id(db: &Connection, name: &str) -> rusqlite::Result<i64> {
        db.execute("INSERT OR IGNORE INTO device (name) VALUES (?1)", [name])?;
        db.query_row("SELECT id FROM device WHERE name = ?1", [name], |r| {
            r.get(0)
        })
    }

    /// Stores `samples` of `device` in one transaction (a second stored
    /// twice keeps the later).
    pub fn add(&self, device: &str, samples: &[Sample]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let mut db = self.conn();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let id = Self::device_id(&tx, device).map_err(|e| e.to_string())?;
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT OR REPLACE INTO second (device, slot, t, peak, late) VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(|e| e.to_string())?;
            for s in samples {
                ins.execute(params![id, s.slot, s.t, s.peak_us, s.late])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    /// Records `flow` as `device`'s slot `slot` from `now` on, when it
    /// differs from the last one recorded.
    pub fn set_flow(
        &self,
        device: &str,
        slot: u16,
        flow: &FlowInfo,
        now: i64,
    ) -> Result<(), String> {
        let db = self.conn();
        let id = Self::device_id(&db, device).map_err(|e| e.to_string())?;
        let last = Self::flow_at(&db, id, slot, i64::MAX).map_err(|e| e.to_string())?;
        if last.as_ref() == Some(flow) {
            return Ok(());
        }
        let json = |v: &Vec<String>| serde_json::to_string(v).unwrap_or_else(|_| "[]".into());
        db.execute(
            "INSERT OR REPLACE INTO flow (device, slot, since, latency_us, rate, channels, sources)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                slot,
                now,
                flow.latency_us,
                flow.rate,
                json(&flow.channels),
                json(&flow.sources)
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn flow_at(db: &Connection, id: i64, slot: u16, at: i64) -> rusqlite::Result<Option<FlowInfo>> {
        db.query_row(
            "SELECT latency_us, rate, channels, sources FROM flow
             WHERE device = ?1 AND slot = ?2 AND since <= ?3 ORDER BY since DESC LIMIT 1",
            params![id, slot, at],
            |r| {
                let list = |s: String| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default();
                Ok(FlowInfo {
                    latency_us: r.get(0)?,
                    rate: r.get(1)?,
                    channels: list(r.get(2)?),
                    sources: list(r.get(3)?),
                })
            },
        )
        .optional()
    }

    /// Rolls completed seconds into minutes and minutes into hours (the
    /// last `window` seconds again, so a late write is counted), then drops
    /// what is past each tier's keep. `now` in unix seconds.
    pub fn rollup(&self, now: i64, window: i64) -> Result<(), String> {
        let minute = now - now.rem_euclid(60);
        let hour = now - now.rem_euclid(3600);
        let db = self.conn();
        db.execute(
            "INSERT OR REPLACE INTO minute (device, slot, t, peak, sum, n, late)
             SELECT device, slot, t - t % 60, MAX(peak), SUM(peak), COUNT(*), SUM(late) FROM second
             WHERE t >= ?1 AND t < ?2 GROUP BY device, slot, t - t % 60",
            params![minute - window - window.rem_euclid(60), minute],
        )
        .map_err(|e| format!("roll seconds: {e}"))?;
        db.execute(
            "INSERT OR REPLACE INTO hour (device, slot, t, peak, sum, n, late)
             SELECT device, slot, t - t % 3600, MAX(peak), SUM(sum), SUM(n), SUM(late) FROM minute
             WHERE t >= ?1 AND t < ?2 GROUP BY device, slot, t - t % 3600",
            params![hour - 3600 - window, hour],
        )
        .map_err(|e| format!("roll minutes: {e}"))?;
        for (table, keep) in [
            ("second", KEEP_SECONDS),
            ("minute", KEEP_MINUTES),
            ("hour", KEEP_HOURS),
        ] {
            db.execute(&format!("DELETE FROM {table} WHERE t < ?1"), [now - keep])
                .map_err(|e| format!("prune {table}: {e}"))?;
        }
        Ok(())
    }

    /// Every receiver with samples in the last hour before `now`: its peak
    /// against its flow's latency, its late packets over the hour and the
    /// day, and a sparkline of the hour.
    pub fn overview(&self, now: i64) -> Result<api::Overview, String> {
        let hour = now - 3600;
        let step = 3600 / api::SPARK_POINTS;
        let db = self.conn();
        let mut out = api::Overview {
            now,
            receivers: Vec::new(),
        };
        let mut q = db
            .prepare(
                "SELECT d.name, d.id, COUNT(DISTINCT s.slot), MAX(s.t) FROM second s JOIN device d ON d.id = s.device
                 WHERE s.t > ?1 AND s.t <= ?2 GROUP BY d.id ORDER BY d.name COLLATE NOCASE",
            )
            .map_err(|e| e.to_string())?;
        let devices: Vec<(String, i64, u16, i64)> = q
            .query_map(params![hour, now], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let late = |id: i64, from: i64| -> Result<u64, String> {
            db.query_row(
                "SELECT COALESCE(SUM(late), 0) FROM second WHERE device = ?1 AND t > ?2 AND t <= ?3",
                params![id, from, now],
                |r| r.get::<_, i64>(0),
            )
            .map(|v| u64::try_from(v).unwrap_or(0))
            .map_err(|e| e.to_string())
        };
        for (name, id, flows, last) in devices {
            let mut r = api::ReceiverSummary {
                device: name,
                flows,
                late_1h: late(id, hour)?,
                late_24h: late(id, now - 24 * 3600)?,
                spark: vec![None; usize::try_from(api::SPARK_POINTS).unwrap_or(0)],
                age_s: Some(now - last),
                ..api::ReceiverSummary::default()
            };
            let mut per = db
                .prepare(
                    "SELECT slot, (t - ?2 - 1) / ?4 AS b, MAX(peak) FROM second
                     WHERE device = ?1 AND t > ?2 AND t <= ?3 GROUP BY slot, b",
                )
                .map_err(|e| e.to_string())?;
            let rows = per
                .query_map(params![id, hour, now, step], |rw| {
                    Ok((
                        rw.get::<_, u16>(0)?,
                        rw.get::<_, i64>(1)?,
                        rw.get::<_, u32>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            let mut latencies: Vec<(u16, u32)> = Vec::new();
            for row in rows {
                let (slot, b, peak) = row.map_err(|e| e.to_string())?;
                let lat = match latencies.iter().find(|(s, _)| *s == slot) {
                    Some((_, l)) => *l,
                    None => {
                        let l = Self::flow_at(&db, id, slot, now)
                            .map_err(|e| e.to_string())?
                            .map_or(0, |f| f.latency_us);
                        latencies.push((slot, l));
                        l
                    }
                };
                if peak >= r.peak_us {
                    r.peak_us = peak;
                    r.latency_us = lat;
                }
                let share = if lat > 0 {
                    u32::try_from(u64::from(peak) * 1000 / u64::from(lat)).unwrap_or(u32::MAX)
                } else {
                    0
                };
                if let Some(cell) = usize::try_from(b).ok().and_then(|i| r.spark.get_mut(i)) {
                    *cell = Some(cell.map_or(share, |c| c.max(share)));
                }
            }
            out.receivers.push(r);
        }
        Ok(out)
    }

    /// The history of `device` from `from` to `to` (unix seconds).
    pub fn history(&self, device: &str, from: i64, to: i64) -> Result<api::History, String> {
        let (from, to) = (from.min(to), from.max(to));
        let (tier, step) = plan(to - from);
        let mut out = api::History {
            device: device.to_owned(),
            from,
            to,
            step_s: step,
            tier_s: tier.unit_s(),
            ..api::History::default()
        };
        let first = from - from.rem_euclid(step);
        out.t = (0..)
            .map(|k| first + k * step)
            .take_while(|&b| b <= to)
            .collect();
        let db = self.conn();
        let Some(id) = db
            .query_row("SELECT id FROM device WHERE name = ?1", [device], |r| {
                r.get::<_, i64>(0)
            })
            .optional()
            .map_err(|e| e.to_string())?
        else {
            return Ok(out);
        };
        let src = tier.source();
        let mut q = db
            .prepare(&format!(
                "SELECT slot, t - (t - ?2) % ?4 AS b, MAX(peak), SUM(sum), SUM(n), SUM(late)
                 FROM ({src}) WHERE t >= ?2 AND t <= ?3
                 GROUP BY slot, b ORDER BY slot, b"
            ))
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map(params![id, first, to, step], |r| {
                Ok((
                    r.get::<_, u16>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, u32>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut flows: Vec<api::FlowHistory> = Vec::new();
        let mut sums: Vec<(i64, i64)> = Vec::new();
        for row in rows {
            let (slot, b, peak, s, cnt, late) = row.map_err(|e| e.to_string())?;
            if flows.last().is_none_or(|f| f.slot != slot) {
                let info = Self::flow_at(&db, id, slot, to)
                    .map_err(|e| e.to_string())?
                    .unwrap_or_default();
                flows.push(api::FlowHistory {
                    slot,
                    latency_us: info.latency_us,
                    rate: info.rate,
                    channels: info.channels,
                    sources: info.sources,
                    max_us: vec![None; out.t.len()],
                    mean_us: vec![None; out.t.len()],
                    late: vec![0; out.t.len()],
                    ..api::FlowHistory::default()
                });
                sums.push((0, 0));
            }
            let (Some(f), Some(acc)) = (flows.last_mut(), sums.last_mut()) else {
                continue;
            };
            let Some(i) = usize::try_from((b - first) / step)
                .ok()
                .filter(|&i| i < out.t.len())
            else {
                continue;
            };
            let late = u64::try_from(late).unwrap_or(0);
            f.max_us[i] = Some(peak);
            f.mean_us[i] = (cnt > 0).then(|| u32::try_from(s / cnt).unwrap_or(u32::MAX));
            f.late[i] = late;
            f.peak_us = f.peak_us.max(peak);
            f.late_total += late;
            acc.0 += s;
            acc.1 += cnt;
        }
        // The distribution and the count past the latency, over the stored values.
        let mut values = db
            .prepare(&format!(
                "SELECT peak FROM ({src}) WHERE slot = ?2 AND t >= ?3 AND t <= ?4"
            ))
            .map_err(|e| e.to_string())?;
        for (f, (s, cnt)) in flows.iter_mut().zip(sums) {
            f.avg_us = if cnt > 0 {
                u32::try_from(s / cnt).unwrap_or(u32::MAX)
            } else {
                0
            };
            let edges = api::latency_bins(f.latency_us);
            f.histogram = vec![0; edges.len() + 1];
            let vals = values
                .query_map(params![id, f.slot, first, to], |r| r.get::<_, u32>(0))
                .map_err(|e| e.to_string())?;
            for v in vals {
                let v = v.map_err(|e| e.to_string())?;
                f.histogram[api::latency_bin(&edges, v)] += 1;
                if f.latency_us > 0 && v > f.latency_us {
                    f.over += 1;
                }
            }
        }
        out.flows = flows;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(slot: u16, t: i64, peak_us: u32, late: u32) -> Sample {
        Sample {
            slot,
            t,
            peak_us,
            late,
        }
    }

    fn flow(latency_us: u32) -> FlowInfo {
        FlowInfo {
            latency_us,
            rate: 48000,
            channels: vec!["In 1".into()],
            sources: vec!["desk".into()],
        }
    }

    #[test]
    fn plan_picks_the_tier_by_span_and_keeps_the_buckets_few() {
        assert_eq!(plan(300), (Tier::Second, 1));
        assert_eq!(plan(3600), (Tier::Second, 3));
        assert_eq!(plan(2 * 3600), (Tier::Second, 5));
        assert_eq!(plan(6 * 3600), (Tier::Minute, 60));
        assert_eq!(plan(7 * 24 * 3600), (Tier::Minute, 420));
        assert_eq!(plan(30 * 24 * 3600), (Tier::Hour, 3600));
        assert_eq!(plan(365 * 24 * 3600), (Tier::Hour, 6 * 3600));
        for span in [1, 59, 7200, 7201, 600_000, 31_536_000] {
            assert!(span / plan(span).1 <= api::HISTORY_POINTS, "span {span}");
        }
    }

    #[test]
    fn seconds_come_back_bucketed_per_flow_with_the_flow_description() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000;
        st.set_flow("desk", 1, &flow(1000), t0 - 10).unwrap();
        st.add(
            "desk",
            &[
                s(1, t0, 100, 0),
                s(1, t0 + 1, 300, 0),
                s(1, t0 + 2, 1200, 3),
                s(2, t0 + 1, 50, 0),
            ],
        )
        .unwrap();
        let h = st.history("desk", t0, t0 + 299).unwrap();
        assert_eq!((h.tier_s, h.step_s), (1, 1));
        assert_eq!(h.t.len(), 300);
        assert_eq!(h.t[0], t0);
        assert_eq!(h.flows.len(), 2);
        let f = &h.flows[0];
        assert_eq!((f.slot, f.latency_us, f.rate), (1, 1000, 48000));
        assert_eq!(f.sources, vec!["desk".to_owned()]);
        assert_eq!(&f.max_us[..4], &[Some(100), Some(300), Some(1200), None]);
        assert_eq!(f.late[2], 3);
        assert_eq!(
            (f.peak_us, f.avg_us, f.late_total, f.over),
            (1200, 533, 3, 1)
        );
        assert_eq!(f.histogram.iter().sum::<u64>(), 3);
        assert_eq!(f.histogram[8], 1); // 1200 µs: past the 1000 µs latency
        // A slot with no description recorded reads latency 0.
        assert_eq!(h.flows[1].latency_us, 0);
        // An unknown device: the time axis, no flows.
        let none = st.history("nobody", t0, t0 + 59).unwrap();
        assert!(none.flows.is_empty());
        assert_eq!(none.t.len(), 60);
    }

    #[test]
    fn buckets_take_the_max_and_the_mean_of_their_seconds() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000; // a whole hour
        let samples: Vec<Sample> = (0..3600)
            .map(|k| s(1, t0 + k, (k % 10) as u32 * 10, 0))
            .collect();
        st.add("desk", &samples).unwrap();
        let h = st.history("desk", t0, t0 + 3599).unwrap();
        assert_eq!(h.step_s, 3);
        let f = &h.flows[0];
        // Bucket 0: seconds 0, 1, 2 -> peaks 0, 10, 20.
        assert_eq!((f.max_us[0], f.mean_us[0]), (Some(20), Some(10)));
        assert_eq!(f.peak_us, 90);
        assert_eq!(f.avg_us, 45);
    }

    #[test]
    fn longer_ranges_include_the_seconds_not_yet_rolled_up() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000; // a whole hour
        let samples: Vec<Sample> = (0..7200)
            .map(|k| s(1, t0 + k, if k == 7100 { 700 } else { 100 }, 0))
            .collect();
        st.add("desk", &samples).unwrap();
        // Nothing rolled up yet: a 6 h range still has every minute.
        let h = st.history("desk", t0, t0 + 6 * 3600).unwrap();
        assert_eq!(h.tier_s, 60);
        assert_eq!(h.flows[0].max_us.iter().flatten().count(), 120);
        assert_eq!(h.flows[0].max_us[118], Some(700));
        // The first hour rolled up, the second not: both read, once each.
        st.rollup(t0 + 3600, 3600).unwrap();
        let h = st.history("desk", t0, t0 + 6 * 3600).unwrap();
        assert_eq!(h.flows[0].max_us.iter().flatten().count(), 120);
        assert_eq!(h.flows[0].mean_us[0], Some(100));
        // A 30 day range: two hours, the second from not-yet-rolled seconds.
        let d = st.history("desk", t0, t0 + 30 * 24 * 3600).unwrap();
        assert_eq!(d.tier_s, 3600);
        assert_eq!(&d.flows[0].max_us[..3], &[Some(100), Some(700), None]);
        assert_eq!(d.flows[0].histogram.iter().sum::<u64>(), 2);
    }

    #[test]
    fn rollup_feeds_minutes_and_hours_and_prunes_old_seconds() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000; // a whole hour
        let samples: Vec<Sample> = (0..7200)
            .map(|k| {
                s(
                    1,
                    t0 + k,
                    if k == 90 { 900 } else { 100 },
                    u32::from(k == 90),
                )
            })
            .collect();
        st.add("desk", &samples).unwrap();
        st.rollup(t0 + 7200, 7200).unwrap();
        // A 6 h range reads minutes: the minute holding second 90 has its peak.
        let h = st.history("desk", t0, t0 + 6 * 3600).unwrap();
        assert_eq!((h.tier_s, h.step_s), (60, 60));
        let f = &h.flows[0];
        assert_eq!(f.max_us[1], Some(900));
        assert_eq!(f.mean_us[1], Some((59 * 100 + 900) / 60));
        assert_eq!(f.late[1], 1);
        assert_eq!(f.max_us[0], Some(100));
        assert_eq!(f.max_us.iter().flatten().count(), 120);
        // A 30 day range reads hours.
        let d = st.history("desk", t0, t0 + 30 * 24 * 3600).unwrap();
        assert_eq!(d.tier_s, 3600);
        assert_eq!(d.flows[0].max_us[0], Some(900));
        assert_eq!(d.flows[0].late_total, 1);
        // A day later the seconds are gone, the minutes stay.
        st.rollup(t0 + 7200 + KEEP_SECONDS + 60, 60).unwrap();
        let gone = st.history("desk", t0, t0 + 3599).unwrap();
        assert!(gone.flows.is_empty());
        let kept = st.history("desk", t0, t0 + 6 * 3600).unwrap();
        assert_eq!(kept.flows[0].max_us[1], Some(900));
    }

    #[test]
    fn a_flow_description_is_stored_only_when_it_changes_and_read_as_of_the_range() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000;
        st.set_flow("desk", 1, &flow(1000), t0).unwrap();
        st.set_flow("desk", 1, &flow(1000), t0 + 5).unwrap();
        st.set_flow("desk", 1, &flow(250), t0 + 100).unwrap();
        st.add("desk", &[s(1, t0 + 1, 100, 0), s(1, t0 + 101, 100, 0)])
            .unwrap();
        let count: i64 = st
            .conn()
            .query_row("SELECT COUNT(*) FROM flow", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            st.history("desk", t0, t0 + 50).unwrap().flows[0].latency_us,
            1000
        );
        assert_eq!(
            st.history("desk", t0, t0 + 200).unwrap().flows[0].latency_us,
            250
        );
    }

    #[test]
    fn the_overview_sums_each_receiver_over_its_flows() {
        let st = Store::memory().unwrap();
        let now = 1_800_000_000;
        st.set_flow("stage", 1, &flow(1000), now - 7200).unwrap();
        st.set_flow("stage", 2, &flow(250), now - 7200).unwrap();
        st.add(
            "stage",
            &[
                s(1, now - 3599, 400, 0),
                s(2, now - 10, 200, 2),
                s(1, now - 5, 300, 0),
                s(1, now - 5000, 100, 5), // over an hour ago: the day's count only
            ],
        )
        .unwrap();
        st.add("old", &[s(1, now - 5000, 100, 0)]).unwrap();
        let o = st.overview(now).unwrap();
        assert_eq!(o.now, now);
        assert_eq!(
            o.receivers.len(),
            1,
            "a receiver silent for an hour is left out"
        );
        let r = &o.receivers[0];
        assert_eq!(r.device, "stage");
        assert_eq!(r.flows, 2);
        assert_eq!((r.peak_us, r.latency_us), (400, 1000));
        assert_eq!((r.late_1h, r.late_24h), (2, 7));
        assert_eq!(r.age_s, Some(5));
        assert_eq!(r.spark.len(), 60);
        assert_eq!(r.spark[0], Some(400)); // 400 of 1000 µs
        assert_eq!(r.spark[59], Some(800)); // 200 of 250 µs beats 300 of 1000
        assert_eq!(r.spark[30], None);
    }

    #[test]
    fn the_schema_is_migrated_once_and_survives_a_reopen() {
        let dir = std::env::temp_dir().join(format!("dc-history-{}", std::process::id()));
        let path = dir.join("h.db");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let st = Store::open(&path).unwrap();
            st.add("desk", &[s(1, 1_800_000_000, 100, 0)]).unwrap();
        }
        let st = Store::open(&path).unwrap();
        let v: i64 = st
            .conn()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, i64::try_from(MIGRATIONS.len()).unwrap());
        assert_eq!(
            st.history("desk", 1_800_000_000, 1_800_000_010)
                .unwrap()
                .flows
                .len(),
            1
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
