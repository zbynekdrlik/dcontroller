//! The latency view's numbers: histogram bins against a flow's latency and
//! the figures shown beside it. Pure, so it is tested on the host.

/// One histogram bin: peaks over `from_us` and at most `to_us` (`None`:
/// no upper edge), and whether it lies past the flow's latency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bin {
    pub from_us: u32,
    pub to_us: Option<u32>,
    pub count: usize,
    pub late: bool,
}

/// The bins for a flow of latency `latency_us`: eight up to the latency,
/// then up to 1.5× and 2×, then everything over.
pub fn bins(peaks_us: &[u32], latency_us: u32) -> Vec<Bin> {
    let l = latency_us.max(8);
    let mut edges: Vec<u32> = (1..=8).map(|k| l * k / 8).collect();
    edges.push(l + l / 2);
    edges.push(l * 2);
    let mut out: Vec<Bin> = Vec::with_capacity(edges.len() + 1);
    let mut from = 0;
    for &to in &edges {
        out.push(Bin {
            from_us: from,
            to_us: Some(to),
            count: 0,
            late: from >= l,
        });
        from = to;
    }
    out.push(Bin {
        from_us: from,
        to_us: None,
        count: 0,
        late: true,
    });
    for &p in peaks_us {
        let i = edges.partition_point(|&e| e < p);
        out[i].count += 1;
    }
    out
}

/// The figures of a flow's peaks: last, largest, mean, and how many
/// seconds had a peak past the latency.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Figures {
    pub last_us: u32,
    pub max_us: u32,
    pub mean_us: u32,
    pub seconds_over: usize,
}

pub fn figures(peaks_us: &[u32], latency_us: u32) -> Figures {
    if peaks_us.is_empty() {
        return Figures::default();
    }
    let sum: u64 = peaks_us.iter().map(|&p| u64::from(p)).sum();
    Figures {
        last_us: *peaks_us.last().unwrap_or(&0),
        max_us: peaks_us.iter().copied().max().unwrap_or(0),
        mean_us: u32::try_from(sum / peaks_us.len() as u64).unwrap_or(u32::MAX),
        seconds_over: peaks_us.iter().filter(|&&p| p > latency_us).count(),
    }
}

/// `µs` as Dante Controller writes it: ms with two decimals.
pub fn ms(us: u32) -> String {
    format!("{:.2} ms", f64::from(us) / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_split_at_eighths_of_the_latency_then_past_it() {
        let b = bins(&[], 1000);
        assert_eq!(b.len(), 11);
        assert_eq!((b[0].from_us, b[0].to_us), (0, Some(125)));
        assert_eq!((b[7].from_us, b[7].to_us), (875, Some(1000)));
        assert!(!b[7].late);
        assert_eq!((b[8].from_us, b[8].to_us), (1000, Some(1500)));
        assert!(b[8].late);
        assert_eq!((b[10].from_us, b[10].to_us), (2000, None));
    }

    #[test]
    fn peaks_land_in_their_bins_an_edge_belongs_below() {
        let b = bins(&[0, 125, 126, 1000, 1001, 5000], 1000);
        assert_eq!(b[0].count, 2); // 0 and the edge 125
        assert_eq!(b[1].count, 1);
        assert_eq!(b[7].count, 1); // 1000: at the latency, not late
        assert_eq!(b[8].count, 1);
        assert_eq!(b[10].count, 1);
        assert_eq!(b.iter().map(|b| b.count).sum::<usize>(), 6);
    }

    #[test]
    fn figures_of_the_peaks() {
        let f = figures(&[200, 400, 1200, 300], 1000);
        assert_eq!(
            f,
            Figures {
                last_us: 300,
                max_us: 1200,
                mean_us: 525,
                seconds_over: 1
            }
        );
        assert_eq!(figures(&[], 1000), Figures::default());
        assert_eq!(ms(1250), "1.25 ms");
    }
}
