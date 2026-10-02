use yuanlai_types::Timestamp;

/// Median of `timestamps`; for an even count, the lower of the two middle values (BLK-07).
///
/// Returns `None` for an empty slice.
#[must_use]
pub fn median_time_past(timestamps: &[Timestamp]) -> Option<Timestamp> {
    let mut sorted = timestamps.to_vec();
    sorted.sort_unstable();
    let index = sorted.len().checked_sub(1)?.checked_div(2)?;
    sorted.get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(values: &[u64]) -> Vec<Timestamp> {
        values.iter().copied().map(Timestamp).collect()
    }

    #[test]
    fn odd_count_takes_middle() {
        assert_eq!(median_time_past(&ts(&[5, 1, 3])), Some(Timestamp(3)));
    }

    #[test]
    fn even_count_takes_lower_middle() {
        assert_eq!(median_time_past(&ts(&[4, 1, 3, 2])), Some(Timestamp(2)));
    }

    #[test]
    fn single_and_empty() {
        assert_eq!(median_time_past(&ts(&[9])), Some(Timestamp(9)));
        assert_eq!(median_time_past(&[]), None);
    }
}
