//! The meters view's numbers: how full a channel's bar is, its colour and
//! its text. Pure, so it is tested on the host.

use dcontroller_api::ChannelLevel;

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

#[cfg(test)]
mod tests {
    use super::*;

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
