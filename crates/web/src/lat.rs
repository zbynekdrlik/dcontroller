//! Latency figures as the page writes them.

/// `µs` as Dante Controller writes it: ms with two decimals.
pub fn ms(us: u32) -> String {
    format!("{:.2} ms", f64::from(us) / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_has_two_decimals() {
        assert_eq!(ms(1250), "1.25 ms");
        assert_eq!(ms(62), "0.06 ms");
    }
}
