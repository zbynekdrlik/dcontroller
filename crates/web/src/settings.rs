//! The setting changes a device offers on the page: latency (Dante
//! Controller's choices within the device's range), sample rate (the rates
//! the device lists) and encoding. Pure, so it is tested on the host.

use dcontroller_api::{Device, Setting};

/// Dante Controller's latency choices, µs.
pub const LATENCIES_US: [u32; 6] = [150, 250, 500, 1000, 2000, 5000];
/// The encodings Dante devices offer, bits.
pub const ENCODINGS: [u32; 3] = [16, 24, 32];

/// Which setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Latency,
    SampleRate,
    Encoding,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Latency => "latency",
            Kind::SampleRate => "sample rate",
            Kind::Encoding => "encoding",
        }
    }

    /// The value as the page writes it.
    pub fn text(self, v: u32) -> String {
        match self {
            Kind::Latency => format!("{:.2} ms", f64::from(v) / 1000.0),
            Kind::SampleRate => format!("{} kHz", f64::from(v) / 1000.0),
            Kind::Encoding => format!("{v} bit"),
        }
    }

    pub fn setting(self, v: u32) -> Setting {
        match self {
            Kind::Latency => Setting::LatencyUs(v),
            Kind::SampleRate => Setting::SampleRateHz(v),
            Kind::Encoding => Setting::EncodingBits(v),
        }
    }

    /// What the change does to the audio, for the dialog.
    pub fn warning(self) -> &'static str {
        match self {
            Kind::Latency => {
                "The audio from this device's receivers may drop out briefly while the latency changes."
            }
            Kind::SampleRate => {
                "Devices at another sample rate cannot exchange audio with this one: its subscriptions stop until the rates match. Some devices apply a new rate only after they restart."
            }
            Kind::Encoding => {
                "Receivers that do not take this encoding stop receiving this device's channels."
            }
        }
    }
}

/// One setting the page can change on a device: its value now and the
/// values to choose from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub device: String,
    pub kind: Kind,
    pub current: Option<u32>,
    pub options: Vec<u32>,
}

impl Ask {
    pub fn title(&self) -> String {
        format!(
            "Change the {} of {} (now {})",
            self.kind.name(),
            self.device,
            self.current
                .map_or("unknown".to_owned(), |v| self.kind.text(v))
        )
    }
}

/// The settings `dev` can have changed, from what it last reported; a
/// setting it does not report offers nothing.
pub fn asks(dev: &Device) -> Vec<Ask> {
    let mut out = Vec::new();
    let ask = |kind, current, options: Vec<u32>| Ask {
        device: dev.name.clone(),
        kind,
        current,
        options,
    };
    if let Some(s) = &dev.settings
        && let (Some(lo), Some(hi)) = (s.latency_min_us, s.latency_max_us)
    {
        let current = s.latency_configured_us;
        let mut options: Vec<u32> = LATENCIES_US
            .into_iter()
            .filter(|l| (lo..=hi).contains(l))
            .collect();
        if let Some(c) = current.filter(|c| !options.contains(c)) {
            options.push(c);
            options.sort_unstable();
        }
        if !options.is_empty() {
            out.push(ask(Kind::Latency, current, options));
        }
    }
    if let Some(i) = &dev.info {
        if !i.sample_rates.is_empty() {
            let current = dev.settings.as_ref().and_then(|s| s.sample_rate);
            out.push(ask(Kind::SampleRate, current, i.sample_rates.clone()));
        }
        if let Some(b) = i.encoding_bits {
            out.push(ask(Kind::Encoding, Some(b), ENCODINGS.to_vec()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcontroller_api::{Info, Settings};

    fn dev(settings: Option<Settings>, info: Option<Info>) -> Device {
        Device {
            name: "desk".into(),
            settings,
            info,
            ..Device::default()
        }
    }

    #[test]
    fn latency_choices_stay_within_the_device_range_and_keep_the_current_one() {
        let d = dev(
            Some(Settings {
                latency_configured_us: Some(333),
                latency_min_us: Some(250),
                latency_max_us: Some(2000),
                sample_rate: Some(48000),
                ..Settings::default()
            }),
            None,
        );
        let a = asks(&d);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, Kind::Latency);
        assert_eq!(a[0].options, vec![250, 333, 500, 1000, 2000]);
        assert_eq!(a[0].title(), "Change the latency of desk (now 0.33 ms)");
        assert_eq!(a[0].kind.setting(500), Setting::LatencyUs(500));
    }

    #[test]
    fn rates_and_encodings_come_from_the_device_info() {
        let d = dev(
            Some(Settings {
                sample_rate: Some(96000),
                ..Settings::default()
            }),
            Some(Info {
                sample_rates: vec![48000, 96000],
                encoding_bits: Some(24),
                ..Info::default()
            }),
        );
        let a = asks(&d);
        // No latency range reported: no latency choice.
        assert_eq!(
            a.iter().map(|a| a.kind).collect::<Vec<_>>(),
            vec![Kind::SampleRate, Kind::Encoding]
        );
        assert_eq!(a[0].current, Some(96000));
        assert_eq!(a[0].options, vec![48000, 96000]);
        assert_eq!(a[0].kind.text(44100), "44.1 kHz");
        assert_eq!(a[1].options, vec![16, 24, 32]);
        assert_eq!(a[1].kind.setting(32), Setting::EncodingBits(32));
        assert!(asks(&dev(None, None)).is_empty());
        assert!(asks(&dev(None, Some(Info::default()))).is_empty());
    }
}
