//! The latency history: every receiver's per-second peak lateness and late
//! packets per RX flow, kept in SQLite in three tiers (seconds for a day,
//! minutes for a month, hours for a year) and read back bucketed for any
//! range (`GET /api/history/{device}`). The recorder (`recorder.rs`) fills
//! it; nothing here talks to a device.

use std::path::Path;
use std::sync::Mutex;

use dcontroller_api as api;
use rusqlite::{Connection, OptionalExtension, params};
use tracing::{info, warn};

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
    // 2: time indexes for the rollup, the pruning and the overview.
    "CREATE INDEX second_t ON second (t);
     CREATE INDEX minute_t ON minute (t);
     CREATE INDEX hour_t ON hour (t);",
    // 3: one row per device name, case aside (Dante compares names so):
    // rows split by case before v0.1.24 are merged into the oldest id.
    "CREATE TEMP TABLE dup AS SELECT d.id AS id,
         (SELECT MIN(e.id) FROM device e WHERE e.name = d.name COLLATE NOCASE) AS keep FROM device d;
     DELETE FROM dup WHERE id = keep;
     INSERT INTO second (device, slot, t, peak, late)
         SELECT dup.keep, s.slot, s.t, s.peak, s.late FROM second s JOIN dup ON dup.id = s.device WHERE true
         ON CONFLICT (device, t, slot) DO UPDATE SET peak = MAX(peak, excluded.peak), late = late + excluded.late;
     INSERT INTO minute (device, slot, t, peak, sum, n, late)
         SELECT dup.keep, m.slot, m.t, m.peak, m.sum, m.n, m.late FROM minute m JOIN dup ON dup.id = m.device WHERE true
         ON CONFLICT (device, t, slot) DO UPDATE SET peak = MAX(peak, excluded.peak), sum = sum + excluded.sum,
             n = n + excluded.n, late = late + excluded.late;
     INSERT INTO hour (device, slot, t, peak, sum, n, late)
         SELECT dup.keep, h.slot, h.t, h.peak, h.sum, h.n, h.late FROM hour h JOIN dup ON dup.id = h.device WHERE true
         ON CONFLICT (device, t, slot) DO UPDATE SET peak = MAX(peak, excluded.peak), sum = sum + excluded.sum,
             n = n + excluded.n, late = late + excluded.late;
     INSERT OR IGNORE INTO flow (device, slot, since, latency_us, rate, channels, sources)
         SELECT dup.keep, f.slot, f.since, f.latency_us, f.rate, f.channels, f.sources FROM flow f JOIN dup ON dup.id = f.device;
     DELETE FROM second WHERE device IN (SELECT id FROM dup);
     DELETE FROM minute WHERE device IN (SELECT id FROM dup);
     DELETE FROM hour WHERE device IN (SELECT id FROM dup);
     DELETE FROM flow WHERE device IN (SELECT id FROM dup);
     DELETE FROM device WHERE id IN (SELECT id FROM dup);
     DROP TABLE dup;
     CREATE UNIQUE INDEX device_name_nocase ON device (name COLLATE NOCASE);",
];

/// `t` rounded up to a whole `unit`.
fn ceil(t: i64, unit: i64) -> i64 {
    (t + unit - 1).div_euclid(unit) * unit
}

/// Now, unix seconds.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

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
/// minutes up to 7 days, hours beyond, and never a tier that has already
/// dropped the range's start (`age_s`: how long ago it was); at most
/// [`api::HISTORY_POINTS`] buckets, each a whole number of the tier's unit.
pub fn plan(span_s: i64, age_s: i64) -> (Tier, i64) {
    let span = span_s.clamp(1, KEEP_HOURS * 2);
    let tier = if span <= 2 * 3600 && age_s <= KEEP_SECONDS {
        Tier::Second
    } else if span <= 7 * 24 * 3600 && age_s <= KEEP_MINUTES {
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

    /// The id of the device named `name`, case aside (as Dante compares
    /// names), made when new.
    fn device_id(db: &Connection, name: &str) -> rusqlite::Result<i64> {
        db.execute(
            "INSERT INTO device (name) SELECT ?1
             WHERE NOT EXISTS (SELECT 1 FROM device WHERE name = ?1 COLLATE NOCASE)",
            [name],
        )?;
        // The device keeps the spelling it was last listed with.
        db.execute(
            "UPDATE device SET name = ?1 WHERE name = ?1 COLLATE NOCASE AND name <> ?1",
            [name],
        )?;
        db.query_row(
            "SELECT id FROM device WHERE name = ?1 COLLATE NOCASE",
            [name],
            |r| r.get(0),
        )
    }

    /// Stores `samples` of `device` in one transaction (a second stored
    /// twice keeps the larger peak and adds the late packets).
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
                    "INSERT INTO second (device, slot, t, peak, late) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT (device, t, slot) DO UPDATE SET
                     peak = MAX(peak, excluded.peak), late = late + excluded.late",
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
        self.roll(now, window, now)
    }

    /// The rollup as of `at` (the window before it), with the prune edges
    /// of the real `now`.
    fn roll(&self, at: i64, window: i64, now: i64) -> Result<(), String> {
        let minute = at - at.rem_euclid(60);
        let hour = at - at.rem_euclid(3600);
        let db = self.conn();
        db.execute(
            "INSERT OR REPLACE INTO minute (device, slot, t, peak, sum, n, late)
             SELECT device, slot, t - t % 60, MAX(peak), SUM(peak), COUNT(*), SUM(late) FROM second
             WHERE t >= ?1 AND t < ?2 GROUP BY device, slot, t - t % 60",
            // Whole minutes only: a partial one would replace a complete row
            // (one starting before the window, or one the prune already cut).
            params![
                ((minute - window).div_euclid(60) * 60).max(ceil(now - KEEP_SECONDS, 60)),
                minute
            ],
        )
        .map_err(|e| format!("roll seconds: {e}"))?;
        db.execute(
            "INSERT OR REPLACE INTO hour (device, slot, t, peak, sum, n, late)
             SELECT device, slot, t - t % 3600, MAX(peak), SUM(sum), SUM(n), SUM(late) FROM minute
             WHERE t >= ?1 AND t < ?2 GROUP BY device, slot, t - t % 3600",
            // Whole hours only, likewise.
            params![
                ((hour - 3600 - window).div_euclid(3600) * 3600)
                    .max(ceil(now - KEEP_MINUTES, 3600)),
                hour
            ],
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

    /// Rolls up everything left unrolled (seconds and minutes from before a
    /// restart or a stall, per device), an hour at a time so the store is
    /// never held long, then as [`Store::rollup`].
    pub fn catch_up(&self, now: i64) -> Result<(), String> {
        let since: Option<i64> = {
            let db = self.conn();
            // Per device: its first second past its last minute, and its
            // first minute past its last hour; the earliest of all.
            db.query_row(
                "SELECT MIN(f) FROM (
                     SELECT MAX(s.first, COALESCE(m.last + 60, s.first)) AS f
                     FROM (SELECT device, MIN(t) AS first FROM second GROUP BY device) s
                     LEFT JOIN (SELECT device, MAX(t) AS last FROM minute GROUP BY device) m ON m.device = s.device
                     UNION ALL
                     SELECT MAX(m.first, COALESCE(h.last + 3600, m.first))
                     FROM (SELECT device, MIN(t) AS first FROM minute GROUP BY device) m
                     LEFT JOIN (SELECT device, MAX(t) AS last FROM hour GROUP BY device) h ON h.device = m.device)",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?
        };
        if let Some(since) = since.map(|t| t.max(now - KEEP_MINUTES)) {
            // From the hour holding the oldest unrolled row, an hour at a
            // time: each call rolls that hour's minutes, then its hour.
            let mut at = since - since.rem_euclid(3600);
            while at < now {
                self.roll((at + 3600).min(now), 3600 + 60, now)?;
                at += 3600;
            }
        }
        self.rollup(now, 600)
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
        let mut late_q = db
            .prepare_cached(
                "SELECT COALESCE(SUM(CASE WHEN t > ?2 THEN late ELSE 0 END), 0), COALESCE(SUM(late), 0),
                        COUNT(DISTINCT CASE WHEN t > ?2 AND late > 0 THEN t END),
                        COUNT(DISTINCT CASE WHEN t > ?4 - 300 AND late > 0 THEN t END)
                 FROM second WHERE device = ?1 AND t > ?3 AND t <= ?4",
            )
            .map_err(|e| e.to_string())?;
        let mut per = db
            .prepare_cached(
                // Each second against the flow its slot carried at that
                // second: a slot re-patched within the hour keeps its old
                // samples on the old latency (0: no flow known).
                "SELECT s.slot, (s.t - ?2 - 1) / ?4 AS b, COALESCE(f.latency_us, 0),
                        MAX(s.peak), CAST(AVG(s.peak) AS INTEGER)
                 FROM second s LEFT JOIN flow f ON f.device = s.device AND f.slot = s.slot
                     AND f.since = (SELECT MAX(since) FROM flow
                                    WHERE device = s.device AND slot = s.slot AND since <= s.t)
                 WHERE s.device = ?1 AND s.t > ?2 AND s.t <= ?3 GROUP BY s.slot, b, f.since",
            )
            .map_err(|e| e.to_string())?;
        for (name, id, flows, last) in devices {
            let (late_1h, late_24h, late_s_1h, late_s_5m): (i64, i64, i64, i64) = late_q
                .query_row(params![id, hour, now - 24 * 3600, now], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .map_err(|e| e.to_string())?;
            let mut r = api::ReceiverSummary {
                device: name,
                flows,
                late_1h: u64::try_from(late_1h).unwrap_or(0),
                late_24h: u64::try_from(late_24h).unwrap_or(0),
                late_s_1h: u64::try_from(late_s_1h).unwrap_or(0),
                late_s_5m: u64::try_from(late_s_5m).unwrap_or(0),
                spark: vec![None; usize::try_from(api::SPARK_POINTS).unwrap_or(0)],
                age_s: Some(now - last),
                ..api::ReceiverSummary::default()
            };
            let rows = per
                .query_map(params![id, hour, now, step], |rw| {
                    Ok((
                        rw.get::<_, i64>(1)?,
                        rw.get::<_, u32>(2)?,
                        rw.get::<_, u32>(3)?,
                        rw.get::<_, u32>(4)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            // The flow closest to its latency (largest share); with no
            // latency known, the largest peak.
            let mut best: Option<(u32, u32)> = None;
            for row in rows {
                let (b, lat, peak, mean) = row.map_err(|e| e.to_string())?;
                let per_mille = |us: u32| {
                    if lat > 0 {
                        u32::try_from(u64::from(us) * 1000 / u64::from(lat)).unwrap_or(u32::MAX)
                    } else {
                        0
                    }
                };
                let share = per_mille(peak);
                if best.is_none_or(|(s, p)| (share, peak) > (s, p)) {
                    best = Some((share, peak));
                    r.peak_us = peak;
                    r.latency_us = lat;
                }
                if let Some(cell) = usize::try_from(b).ok().and_then(|i| r.spark.get_mut(i)) {
                    // The typical second (mean of the per-second peaks), as
                    // the chart draws it; the worst second is `peak_us`.
                    let typical = per_mille(mean);
                    *cell = Some(cell.map_or(typical, |c| c.max(typical)));
                }
            }
            out.receivers.push(r);
        }
        Ok(out)
    }

    /// The history of `device` from `from` to `to` (unix seconds), asked
    /// at `now`. The range is clamped to what is kept (a year back, an hour
    /// ahead), so any request stays small.
    /// Every source device in the flow table (never pruned), first seen
    /// first, case aside.
    fn source_order(db: &Connection) -> rusqlite::Result<Vec<String>> {
        let mut q = db.prepare("SELECT sources FROM flow ORDER BY since, device, slot")?;
        let rows = q.query_map([], |r| r.get::<_, String>(0))?;
        let mut order: Vec<String> = Vec::new();
        for text in rows {
            let text = text?;
            let sources: Vec<String> = serde_json::from_str(&text).unwrap_or_else(|e| {
                warn!(sources = %text, error = %e, "history: flow sources not readable");
                Vec::new()
            });
            for s in sources {
                if !order.iter().any(|o| o.eq_ignore_ascii_case(&s)) {
                    order.push(s);
                }
            }
        }
        Ok(order)
    }

    pub fn history(
        &self,
        device: &str,
        from: i64,
        to: i64,
        now: i64,
    ) -> Result<api::History, String> {
        let (lo, hi) = (
            now.saturating_sub(KEEP_HOURS + 3600),
            now.saturating_add(3600),
        );
        let (from, to) = (from.min(to).clamp(lo, hi), from.max(to).clamp(lo, hi));
        let (tier, step) = plan(to - from, now - from);
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
        out.source_order = Self::source_order(&db).map_err(|e| e.to_string())?;
        let Some(id) = db
            .query_row(
                "SELECT id FROM device WHERE name = ?1 COLLATE NOCASE",
                [device],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
        else {
            return Ok(out);
        };
        // Each stored value with the flow its slot carried at its time (the
        // `since` of that description; before the first one, the first one):
        // a slot whose flow changed in the range reads as one series per flow.
        let src = format!(
            "SELECT s.*, COALESCE(
                 (SELECT MAX(since) FROM flow WHERE device = ?1 AND slot = s.slot AND since <= s.t),
                 (SELECT MIN(since) FROM flow WHERE device = ?1 AND slot = s.slot), 0) AS seg
             FROM ({}) s",
            tier.source()
        );
        let mut q = db
            .prepare(&format!(
                "SELECT slot, seg, t - (t - ?2) % ?4 AS b, MAX(peak), SUM(sum), SUM(n), SUM(late)
                 FROM ({src}) WHERE t >= ?2 AND t <= ?3
                 GROUP BY slot, seg, b ORDER BY slot, seg, b"
            ))
            .map_err(|e| e.to_string())?;
        let rows = q
            .query_map(params![id, first, to, step], |r| {
                Ok((
                    (r.get::<_, u16>(0)?, r.get::<_, i64>(1)?),
                    r.get::<_, i64>(2)?,
                    r.get::<_, u32>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut flows: Vec<api::FlowHistory> = Vec::new();
        let mut sums: Vec<(i64, i64)> = Vec::new();
        let mut segs: Vec<i64> = Vec::new();
        for row in rows {
            let ((slot, seg), b, peak, s, cnt, late) = row.map_err(|e| e.to_string())?;
            if flows.last().is_none_or(|f| f.slot != slot) || segs.last() != Some(&seg) {
                let info = Self::flow_at(&db, id, slot, seg)
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
                segs.push(seg);
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
                "SELECT peak FROM ({src}) WHERE slot = ?2 AND seg = ?5 AND t >= ?3 AND t <= ?4"
            ))
            .map_err(|e| e.to_string())?;
        for ((f, (s, cnt)), seg) in flows.iter_mut().zip(sums).zip(segs) {
            f.avg_us = if cnt > 0 {
                u32::try_from(s / cnt).unwrap_or(u32::MAX)
            } else {
                0
            };
            let edges = api::latency_bins(f.latency_us);
            f.histogram = vec![0; edges.len() + 1];
            let vals = values
                .query_map(params![id, f.slot, first, to, seg], |r| r.get::<_, u32>(0))
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
    fn source_devices_keep_their_first_seen_order_on_every_receiver() {
        let st = Store::memory().unwrap();
        let t0 = 1_000_000;
        let src = |s: &str, latency_us| FlowInfo {
            sources: vec![s.into()],
            ..flow(latency_us)
        };
        st.set_flow("desk", 1, &src("tascam", 1000), t0).unwrap();
        st.set_flow("desk", 2, &src("master", 1000), t0 + 10)
            .unwrap();
        st.set_flow("rack", 1, &src("MASTER", 1000), t0 + 20)
            .unwrap();
        st.set_flow("rack", 2, &src("band", 1000), t0 + 30).unwrap();
        let order = |device: &str| {
            st.history(device, t0, t0 + 60, t0 + 60)
                .unwrap()
                .source_order
        };
        assert_eq!(order("desk"), vec!["tascam", "master", "band"]);
        assert_eq!(order("rack"), order("desk"));
        assert_eq!(order("nobody"), order("desk"));
    }

    #[test]
    fn plan_picks_the_tier_by_span_and_keeps_the_buckets_few() {
        assert_eq!(plan(300, 0), (Tier::Second, 1));
        assert_eq!(plan(3600, 0), (Tier::Second, 3));
        assert_eq!(plan(2 * 3600, 0), (Tier::Second, 5));
        assert_eq!(plan(6 * 3600, 0), (Tier::Minute, 60));
        assert_eq!(plan(7 * 24 * 3600, 0), (Tier::Minute, 420));
        assert_eq!(plan(30 * 24 * 3600, 0), (Tier::Hour, 3600));
        assert_eq!(plan(365 * 24 * 3600, 0), (Tier::Hour, 6 * 3600));
        for span in [1, 59, 7200, 7201, 600_000, 31_536_000] {
            assert!(span / plan(span, 0).1 <= api::HISTORY_POINTS, "span {span}");
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
        let h = st.history("desk", t0, t0 + 299, t0 + 299).unwrap();
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
        let none = st.history("nobody", t0, t0 + 59, t0 + 59).unwrap();
        assert!(none.flows.is_empty());
        assert_eq!(none.t.len(), 60);
    }

    #[test]
    fn a_slot_whose_flow_changed_in_the_range_reads_as_one_series_per_flow() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000;
        let src = |s: &str, latency_us| FlowInfo {
            sources: vec![s.into()],
            ..flow(latency_us)
        };
        st.set_flow("desk", 1, &src("tascam", 1000), t0 - 10)
            .unwrap();
        st.set_flow("desk", 1, &src("master", 2000), t0 + 100)
            .unwrap();
        st.add(
            "desk",
            &[
                s(1, t0 + 1, 1500, 4),
                s(1, t0 + 99, 300, 0),
                s(1, t0 + 100, 2500, 7),
            ],
        )
        .unwrap();
        let h = st.history("desk", t0, t0 + 299, t0 + 299).unwrap();
        assert_eq!(h.flows.len(), 2);
        let (old, new) = (&h.flows[0], &h.flows[1]);
        // Each sample is under the flow the slot carried at its time.
        assert_eq!(
            (old.slot, old.sources.clone()),
            (1, vec!["tascam".to_owned()])
        );
        assert_eq!(old.latency_us, 1000);
        assert_eq!(
            (old.max_us[1], old.max_us[99], old.max_us[100]),
            (Some(1500), Some(300), None)
        );
        assert_eq!((old.peak_us, old.late_total, old.over), (1500, 4, 1));
        assert_eq!(old.histogram.iter().sum::<u64>(), 2);
        assert_eq!(
            (new.slot, new.sources.clone()),
            (1, vec!["master".to_owned()])
        );
        assert_eq!(new.latency_us, 2000);
        assert_eq!((new.max_us[1], new.max_us[100]), (None, Some(2500)));
        assert_eq!((new.peak_us, new.late_total, new.over), (2500, 7, 1));
        assert_eq!(new.histogram.iter().sum::<u64>(), 1);
    }

    #[test]
    fn samples_before_a_slots_first_description_take_that_description() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000;
        st.add("desk", &[s(1, t0 + 1, 100, 0), s(1, t0 + 200, 100, 0)])
            .unwrap();
        st.set_flow("desk", 1, &flow(1000), t0 + 100).unwrap();
        let h = st.history("desk", t0, t0 + 299, t0 + 299).unwrap();
        assert_eq!(h.flows.len(), 1);
        assert_eq!(h.flows[0].latency_us, 1000);
        assert_eq!(h.flows[0].histogram.iter().sum::<u64>(), 2);
    }

    #[test]
    fn buckets_take_the_max_and_the_mean_of_their_seconds() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000; // a whole hour
        let samples: Vec<Sample> = (0..3600)
            .map(|k| s(1, t0 + k, (k % 10) as u32 * 10, 0))
            .collect();
        st.add("desk", &samples).unwrap();
        let h = st.history("desk", t0, t0 + 3599, t0 + 3599).unwrap();
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
        let h = st
            .history("desk", t0, t0 + 6 * 3600, t0 + 6 * 3600)
            .unwrap();
        assert_eq!(h.tier_s, 60);
        assert_eq!(h.flows[0].max_us.iter().flatten().count(), 120);
        assert_eq!(h.flows[0].max_us[118], Some(700));
        // The first hour rolled up, the second not: both read, once each.
        st.rollup(t0 + 3600, 3600).unwrap();
        let h = st
            .history("desk", t0, t0 + 6 * 3600, t0 + 6 * 3600)
            .unwrap();
        assert_eq!(h.flows[0].max_us.iter().flatten().count(), 120);
        assert_eq!(h.flows[0].mean_us[0], Some(100));
        // A 30 day range: two hours, the second from not-yet-rolled seconds.
        let d = st
            .history("desk", t0, t0 + 30 * 24 * 3600, t0 + 30 * 24 * 3600)
            .unwrap();
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
        let h = st
            .history("desk", t0, t0 + 6 * 3600, t0 + 6 * 3600)
            .unwrap();
        assert_eq!((h.tier_s, h.step_s), (60, 60));
        let f = &h.flows[0];
        assert_eq!(f.max_us[1], Some(900));
        assert_eq!(f.mean_us[1], Some((59 * 100 + 900) / 60));
        assert_eq!(f.late[1], 1);
        assert_eq!(f.max_us[0], Some(100));
        assert_eq!(f.max_us.iter().flatten().count(), 120);
        // A 30 day range reads hours.
        let d = st
            .history("desk", t0, t0 + 30 * 24 * 3600, t0 + 30 * 24 * 3600)
            .unwrap();
        assert_eq!(d.tier_s, 3600);
        assert_eq!(d.flows[0].max_us[0], Some(900));
        assert_eq!(d.flows[0].late_total, 1);
        // A day later the seconds are gone, the minutes stay.
        st.rollup(t0 + 7200 + KEEP_SECONDS + 60, 60).unwrap();
        // Asked as of then, an old hour reads minutes: the seconds are gone.
        let later = t0 + 7200 + KEEP_SECONDS + 60;
        let gone = st.history("desk", t0, t0 + 3599, later).unwrap();
        assert_eq!(gone.tier_s, 60);
        let secs: i64 = st
            .conn()
            .query_row("SELECT COUNT(*) FROM second", [], |r| r.get(0))
            .unwrap();
        assert_eq!(secs, 0);
        let kept = st
            .history("desk", t0, t0 + 6 * 3600, t0 + 6 * 3600)
            .unwrap();
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
            st.history("desk", t0, t0 + 50, t0 + 50).unwrap().flows[0].latency_us,
            1000
        );
        // Over both: each description holds its own samples.
        let both = st.history("desk", t0, t0 + 200, t0 + 200).unwrap();
        let lat: Vec<u32> = both.flows.iter().map(|f| f.latency_us).collect();
        assert_eq!(lat, vec![1000, 250]);
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
                s(1, now - 10, 300, 1), // the same second: counted once
                s(1, now - 1000, 300, 4),
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
        // The flow closest to its latency: 200 of 250 µs (80 %), not 400 of 1000.
        assert_eq!((r.peak_us, r.latency_us), (200, 250));
        assert_eq!((r.late_1h, r.late_24h), (7, 12));
        // Seconds with late packets: two in the hour, one in the last 5 min.
        assert_eq!((r.late_s_1h, r.late_s_5m), (2, 1));
        assert_eq!(r.age_s, Some(5));
        assert_eq!(r.spark.len(), 60);
        assert_eq!(r.spark[0], Some(400)); // 400 of 1000 µs
        assert_eq!(r.spark[59], Some(800)); // 200 of 250 µs beats 300 of 1000
        assert_eq!(r.spark[30], None);
    }

    #[test]
    fn overview_sparkline_is_the_typical_second_as_the_chart() {
        // The chart draws the mean of the per-second peaks; the list's
        // sparkline must draw the same, or one rare second over the latency
        // shows as "exceeded" beside a chart under its line (#50).
        let st = Store::memory().unwrap();
        let now = 1_800_000_000;
        st.set_flow("iem", 1, &flow(250), now - 7200).unwrap();
        st.add(
            "iem",
            &[
                s(1, now - 3599, 150, 0),
                s(1, now - 3598, 150, 0),
                s(1, now - 3597, 300, 0), // one rare second past 250 µs
            ],
        )
        .unwrap();
        let r = &st.overview(now).unwrap().receivers[0];
        assert_eq!(
            r.spark[0],
            Some(800),
            "mean 200 of 250 µs, not the worst 300"
        );
        // The worst peak stays reported on its own.
        assert_eq!((r.peak_us, r.latency_us), (300, 250));
    }

    #[test]
    fn overview_sparkline_takes_the_latency_of_the_flow_at_that_time() {
        // A slot re-patched within the hour: its old samples belong to the
        // old flow's latency, not today's (#50: 2.2 ms of an 8.4 ms flow
        // read as 880 % of a 0.25 ms one).
        let st = Store::memory().unwrap();
        let now = 1_800_000_000;
        st.set_flow("iem", 1, &flow(8000), now - 7200).unwrap();
        st.set_flow("iem", 1, &flow(250), now - 1800).unwrap();
        st.add("iem", &[s(1, now - 3599, 2000, 0), s(1, now - 10, 200, 0)])
            .unwrap();
        let r = &st.overview(now).unwrap().receivers[0];
        assert_eq!(r.spark[0], Some(250), "2000 of the old 8000 µs");
        assert_eq!(r.spark[59], Some(800), "200 of today's 250 µs");
        assert_eq!((r.peak_us, r.latency_us), (200, 250));
    }

    #[test]
    fn device_names_match_case_aside() {
        // The recorder once stored samples under "stage-1" and flows under
        // "Stage-1": two device rows, and neither page found both.
        let st = Store::memory().unwrap();
        let now = 1_800_000_000;
        st.set_flow("Stage-1", 1, &flow(1000), now - 60).unwrap();
        st.add("stage-1", &[s(1, now - 5, 400, 0)]).unwrap();
        let ids: i64 = st
            .conn()
            .query_row("SELECT COUNT(*) FROM device", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ids, 1);
        let h = st.history("Stage-1", now - 60, now, now).unwrap();
        assert_eq!(h.flows[0].latency_us, 1000);
        assert_eq!(h.flows[0].peak_us, 400);
        assert_eq!(st.overview(now).unwrap().receivers[0].latency_us, 1000);
    }

    #[test]
    fn the_rollup_every_minute_keeps_whole_hours_and_minutes() {
        // The recorder rolls up every minute with a 600 s window; the hour
        // row must still hold all its 3600 seconds afterwards.
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000; // a whole hour
        let samples: Vec<Sample> = (0..7200).map(|k| s(1, t0 + k, 100, 1)).collect();
        st.add("desk", &samples).unwrap();
        for m in 1..=125 {
            st.rollup(t0 + m * 60 + 7, 600).unwrap();
        }
        let (n, late): (i64, i64) = st
            .conn()
            .query_row("SELECT n, late FROM hour WHERE t = ?1", [t0], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((n, late), (3600, 3600));
        let mins: i64 = st
            .conn()
            .query_row("SELECT MIN(n) FROM minute", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mins, 60);
    }

    #[test]
    fn a_second_stored_twice_keeps_its_peak_and_adds_its_late_packets() {
        let st = Store::memory().unwrap();
        st.add("desk", &[s(1, 100, 700, 3)]).unwrap();
        st.add("desk", &[s(1, 100, 200, 2)]).unwrap();
        let (peak, late): (i64, i64) = st
            .conn()
            .query_row("SELECT peak, late FROM second", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((peak, late), (700, 5));
    }

    #[test]
    fn an_extreme_range_is_clamped_and_stays_small() {
        let st = Store::memory().unwrap();
        let now = 1_800_000_000;
        for (from, to) in [
            (i64::MIN, 0),
            (0, i64::MAX),
            (i64::MIN, i64::MAX),
            (now, now - 10),
        ] {
            let h = st.history("desk", from, to, now).unwrap();
            assert!(
                h.t.len() as i64 <= api::HISTORY_POINTS + 1,
                "{from}..{to}: {}",
                h.t.len()
            );
            assert!(
                h.from >= now - KEEP_HOURS - 3600 && h.to <= now + 3600,
                "{from}..{to}"
            );
        }
    }

    #[test]
    fn an_old_short_range_reads_a_tier_that_still_holds_it() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000;
        let samples: Vec<Sample> = (0..3600).map(|k| s(1, t0 + k, 100, 0)).collect();
        st.add("desk", &samples).unwrap();
        st.rollup(t0 + 3600 + 60, 3600 + 60).unwrap();
        // Three days later the seconds are pruned; an hour from then reads minutes.
        let now = t0 + 3 * 24 * 3600;
        st.rollup(now, 60).unwrap();
        let h = st.history("desk", t0, t0 + 3599, now).unwrap();
        assert_eq!(h.tier_s, 60);
        assert_eq!(h.flows[0].max_us.iter().flatten().count(), 60);
    }

    #[test]
    fn seconds_left_from_before_a_restart_are_rolled_up_on_start() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000;
        let samples: Vec<Sample> = (0..600).map(|k| s(1, t0 + k, 100, 0)).collect();
        st.add("desk", &samples).unwrap();
        // Back after an hour: a normal rollup covers only its window.
        let now = t0 + 3600;
        st.add("desk", &[s(1, now - 30, 100, 0)]).unwrap();
        st.catch_up(now).unwrap();
        let mins: i64 = st
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM minute WHERE t < ?1",
                [t0 + 600],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mins, 10);
    }

    #[test]
    fn a_rollup_near_the_prune_edge_never_replaces_a_whole_minute_with_a_part() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000;
        let samples: Vec<Sample> = (0..120).map(|k| s(1, t0 + k, 100, 0)).collect();
        st.add("desk", &samples).unwrap();
        st.rollup(t0 + 180, 600).unwrap();
        // A day later, 30 s into the first minute: its first 30 seconds are
        // pruned; a catch-up over the whole day must leave the minute whole.
        let now = t0 + KEEP_SECONDS + 30;
        st.rollup(now, 600).unwrap(); // prunes the minute's first 30 seconds
        st.rollup(now, KEEP_SECONDS).unwrap();
        let n: i64 = st
            .conn()
            .query_row("SELECT n FROM minute WHERE t = ?1", [t0], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 60);
    }

    #[test]
    fn catch_up_reaches_each_devices_own_unrolled_seconds() {
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000;
        // B has seconds from before a two-hour stop; A has fresh minutes.
        st.add(
            "b",
            &(0..300).map(|k| s(1, t0 + k, 100, 0)).collect::<Vec<_>>(),
        )
        .unwrap();
        let now = t0 + 7200;
        st.add(
            "a",
            &(0..300)
                .map(|k| s(1, now - 400 + k, 100, 0))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        st.rollup(now, 600).unwrap();
        st.catch_up(now).unwrap();
        let b: i64 = st
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM minute m JOIN device d ON d.id = m.device WHERE d.name = 'b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(b, 5);
    }

    #[test]
    fn device_rows_split_by_case_are_merged_by_the_schema() {
        // Databases written before the fix may hold "stage-1" and "Stage-1".
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(&format!("{}; PRAGMA user_version = 1;", MIGRATIONS[0]))
            .unwrap();
        db.execute_batch(
            "INSERT INTO device (id, name) VALUES (1, 'Stage-1'), (2, 'stage-1');
             INSERT INTO flow VALUES (1, 1, 10, 1000, 48000, '[]', '[]');
             INSERT INTO second VALUES (2, 1, 20, 400, 3), (1, 1, 20, 100, 1);",
        )
        .unwrap();
        let st = Store::with(db).unwrap();
        let ids: i64 = st
            .conn()
            .query_row("SELECT COUNT(*) FROM device", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ids, 1);
        let h = st.history("stage-1", 0, 100, 100).unwrap();
        assert_eq!(h.flows.len(), 1);
        assert_eq!((h.flows[0].latency_us, h.flows[0].peak_us), (1000, 400));
        // The schema refuses a second spelling from now on.
        assert!(
            st.conn()
                .execute("INSERT INTO device (name) VALUES ('STAGE-1')", [])
                .is_err()
        );
    }

    #[test]
    fn catch_up_builds_the_hours_left_from_before_a_long_stop() {
        // Stopped at :30 with minutes but no hour row; back three days later.
        let st = Store::memory().unwrap();
        let t0 = 1_800_000_000; // a whole hour
        st.add(
            "desk",
            &(0..1800).map(|k| s(1, t0 + k, 100, 1)).collect::<Vec<_>>(),
        )
        .unwrap();
        st.rollup(t0 + 1800, 1800).unwrap();
        let now = t0 + 3 * 24 * 3600;
        st.catch_up(now).unwrap();
        let (n, late): (i64, i64) = st
            .conn()
            .query_row("SELECT n, late FROM hour WHERE t = ?1", [t0], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((n, late), (1800, 1800));
    }

    #[test]
    fn a_device_keeps_its_latest_spelling() {
        let st = Store::memory().unwrap();
        st.add("stage-1", &[s(1, 100, 1, 0)]).unwrap();
        st.add("Stage-1", &[s(1, 101, 1, 0)]).unwrap();
        let name: String = st
            .conn()
            .query_row("SELECT name FROM device", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Stage-1");
        assert_eq!(st.overview(200).unwrap().receivers[0].device, "Stage-1");
    }

    #[test]
    fn time_indexes_serve_the_rollup_and_pruning() {
        let st = Store::memory().unwrap();
        let names: Vec<String> = st
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for i in ["hour_t", "minute_t", "second_t"] {
            assert!(names.contains(&i.to_owned()), "{i} in {names:?}");
        }
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
            st.history("desk", 1_800_000_000, 1_800_000_010, 1_800_000_010)
                .unwrap()
                .flows
                .len(),
            1
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
