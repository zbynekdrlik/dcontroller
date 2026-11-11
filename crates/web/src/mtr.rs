//! The meters view's numbers: how full a channel's bar is, its colour and
//! its text. Pure, so it is tested on the host.

use std::collections::HashMap;

use dcontroller_api::{ChannelLevel, MeterReport};

/// The bottom of the bar, dBFS.
pub const FLOOR_DB: f32 = -60.0;
/// From here a level reads hot.
pub const HOT_DB: f32 = -9.0;

/// The bar's fill, 0 to 100 (%).
pub fn fill(l: &ChannelLevel) -> f32 {
    match l.dbfs {
        Some(d) if !l.muted => ((d - FLOOR_DB) / -FLOOR_DB * 100.0).clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// The bar's CSS class.
pub fn class(l: &ChannelLevel) -> &'static str {
    match (l.clip, l.muted, l.dbfs) {
        (true, _, _) => "meter clip",
        (_, true, _) => "meter mute",
        (_, _, Some(d)) if d >= HOT_DB => "meter hot",
        (_, _, Some(_)) => "meter ok",
        _ => "meter none",
    }
}

/// The level as text.
pub fn text(l: &ChannelLevel) -> String {
    match (l.clip, l.muted, l.dbfs) {
        (true, _, _) => "CLIP".to_owned(),
        (_, true, _) => "muted".to_owned(),
        (_, _, Some(d)) if d <= FLOOR_DB => format!("< {FLOOR_DB:.0} dB"),
        (_, _, Some(d)) => format!("{d:.1} dB"),
        _ => "–".to_owned(),
    }
}

/// How loud a level is, for picking the loudest: clipping beats any
/// level, a level beats silence, muted and unsent.
fn loudness(l: &ChannelLevel) -> f32 {
    match (l.clip, l.muted, l.dbfs) {
        (true, _, _) => f32::INFINITY,
        (_, false, Some(d)) => d,
        _ => f32::NEG_INFINITY,
    }
}

/// The level a matrix header shows: channel `channel` (by number), or for
/// a folded device (`None`) its loudest channel. `None` when there is no
/// such channel.
pub fn pick(levels: &[ChannelLevel], channel: Option<u16>) -> Option<&ChannelLevel> {
    match channel {
        Some(n) => levels.iter().find(|l| l.channel == n),
        None => levels
            .iter()
            .max_by(|a, b| loudness(a).total_cmp(&loudness(b))),
    }
}

/// A receiver's own RX levels, or `None` when it sends no meter frames
/// (no report, no frame, or no RX levels in them).
pub fn own_rx(r: Option<&MeterReport>) -> Option<&[ChannelLevel]> {
    r.filter(|r| r.frames > 0 && !r.rx.is_empty())
        .map(|r| r.rx.as_slice())
}

/// The loudest of the sources' TX levels (`(device lower case, TX channel
/// number)`): what a receiver that sends no meters is fed.
pub fn source_level(
    reports: &HashMap<String, MeterReport>,
    sources: &[(String, u16)],
) -> Option<ChannelLevel> {
    sources
        .iter()
        .filter_map(|(dev, ch)| {
            reports
                .get(dev)
                .filter(|r| r.frames > 0)
                .and_then(|r| r.tx.iter().find(|l| l.channel == *ch))
        })
        .max_by(|a, b| loudness(a).total_cmp(&loudness(b)))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_receiver_without_meters_is_shown_at_its_source_level() {
        let lvl = |channel: u16, dbfs: Option<f32>| ChannelLevel {
            channel,
            label: format!("{channel:02}"),
            dbfs,
            clip: false,
            muted: dbfs.is_none(),
        };
        let report =
            |device: &str, frames: u64, tx: Vec<ChannelLevel>, rx: Vec<ChannelLevel>| MeterReport {
                device: device.into(),
                frames,
                tx,
                rx,
                ..MeterReport::default()
            };
        let mut m = HashMap::new();
        m.insert(
            "songplayer".to_string(),
            report(
                "songplayer",
                5,
                vec![lvl(1, Some(-27.5)), lvl(2, Some(-40.0))],
                vec![],
            ),
        );
        m.insert(
            "repraciky".to_string(),
            report("repraciky", 0, vec![], vec![]),
        );
        m.insert(
            "desk".to_string(),
            report("desk", 9, vec![], vec![lvl(1, Some(-12.0))]),
        );
        // No frames from the speakers: their own RX is unknown.
        assert!(own_rx(m.get("repraciky")).is_none());
        assert!(own_rx(None).is_none());
        assert_eq!(own_rx(m.get("desk")).map(<[_]>::len), Some(1));
        let src = |s: &[(&str, u16)]| {
            s.iter()
                .map(|(d, c)| ((*d).to_string(), *c))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            source_level(&m, &src(&[("songplayer", 2)])).and_then(|l| l.dbfs),
            Some(-40.0)
        );
        // A folded receiver: the loudest of its sources.
        assert_eq!(
            source_level(&m, &src(&[("songplayer", 2), ("songplayer", 1)])).and_then(|l| l.dbfs),
            Some(-27.5)
        );
        // A source without meters, or no source: nothing.
        assert!(source_level(&m, &src(&[("repraciky", 1)])).is_none());
        assert!(source_level(&m, &[]).is_none());
    }

    fn at(dbfs: Option<f32>, clip: bool, muted: bool) -> ChannelLevel {
        ChannelLevel {
            channel: 1,
            label: "Vox".into(),
            dbfs,
            clip,
            muted,
        }
    }

    #[test]
    fn a_folded_device_shows_its_loudest_channel_and_an_open_one_each() {
        let ch = |n: u16, dbfs: Option<f32>, clip: bool, muted: bool| ChannelLevel {
            channel: n,
            label: format!("Ch {n}"),
            dbfs,
            clip,
            muted,
        };
        let l = vec![
            ch(1, Some(-40.0), false, false),
            ch(2, Some(-12.0), false, false),
            ch(3, Some(0.0), false, true),
            ch(4, None, false, false),
        ];
        assert_eq!(pick(&l, None).map(|x| x.channel), Some(2));
        assert_eq!(pick(&l, Some(1)).map(|x| x.channel), Some(1));
        assert_eq!(pick(&l, Some(9)), None);
        assert_eq!(pick(&[], None), None);
        let mut c = l.clone();
        c.push(ch(5, Some(0.0), true, false));
        assert_eq!(pick(&c, None).map(|x| x.channel), Some(5));
        // All silent or muted: the folded bar still has a channel, at no level.
        let quiet = vec![ch(1, None, false, true), ch(2, None, false, false)];
        assert_eq!(fill(pick(&quiet, None).unwrap()), 0.0);
    }

    #[test]
    fn bars_fill_from_minus_60_db_and_colour_by_level() {
        assert_eq!(fill(&at(Some(0.0), false, false)), 100.0);
        assert_eq!(fill(&at(Some(-30.0), false, false)), 50.0);
        assert_eq!(fill(&at(Some(-90.0), false, false)), 0.0);
        assert_eq!(fill(&at(None, false, true)), 0.0);
        assert_eq!(class(&at(Some(0.0), true, false)), "meter clip");
        assert_eq!(class(&at(Some(-6.0), false, false)), "meter hot");
        assert_eq!(class(&at(Some(-20.0), false, false)), "meter ok");
        assert_eq!(class(&at(None, false, true)), "meter mute");
        assert_eq!(class(&at(None, false, false)), "meter none");
        assert_eq!(text(&at(Some(0.0), true, false)), "CLIP");
        assert_eq!(text(&at(Some(-20.0), false, false)), "-20.0 dB");
        assert_eq!(text(&at(Some(-80.0), false, false)), "< -60 dB");
        assert_eq!(text(&at(None, false, true)), "muted");
        assert_eq!(text(&at(None, false, false)), "–");
    }
}
