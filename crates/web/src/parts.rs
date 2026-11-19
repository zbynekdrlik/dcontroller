//! A page made of parts (#123): Clock and Network each hold a few parts
//! that have nothing to do with one another. The parts in trouble come
//! first, and so do the rows in trouble inside a table. Pure, tested on
//! the host.

/// Where a state sorts: `bad`, then `warn`, then the rest.
pub fn rank(state: &str) -> u8 {
    match state {
        "bad" => 0,
        "warn" => 1,
        _ => 2,
    }
}

/// The parts' places by their `states`: the ones in trouble first, each
/// group in the order given.
pub fn order(states: &[&str]) -> Vec<usize> {
    let mut v: Vec<usize> = (0..states.len()).collect();
    v.sort_by_key(|&i| rank(states[i]));
    v
}

/// `rows` with the ones in trouble first, each group in the order given.
pub fn trouble_first<T>(rows: Vec<T>, bad: impl Fn(&T) -> bool) -> Vec<T> {
    let (mut first, rest): (Vec<T>, Vec<T>) = rows.into_iter().partition(|r| bad(r));
    first.extend(rest);
    first
}

/// A state in a word, for a part's chip.
pub fn word(state: &str) -> &'static str {
    match state {
        "bad" => "in trouble",
        "warn" => "check",
        "ok" => "ok",
        _ => "no data",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parts_in_trouble_come_first_and_the_rest_keep_their_order() {
        assert_eq!(order(&["ok", "none", "ok"]), [0, 1, 2]);
        assert_eq!(order(&["ok", "warn", "none", "bad"]), [3, 1, 0, 2]);
        // Two in the same trouble keep their order.
        assert_eq!(order(&["bad", "ok", "bad"]), [0, 2, 1]);
        assert!(order(&[]).is_empty());
    }

    #[test]
    fn the_rows_in_trouble_come_first_and_each_group_keeps_its_order() {
        let rows = vec![("a", false), ("b", true), ("c", false), ("d", true)];
        let names: Vec<&str> = trouble_first(rows, |r| r.1)
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(names, ["b", "d", "a", "c"]);
    }

    #[test]
    fn a_state_reads_as_a_word() {
        assert_eq!(
            ["bad", "warn", "ok", "none"].map(word),
            ["in trouble", "check", "ok", "no data"]
        );
    }
}
