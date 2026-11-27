//! Lexicographic ranks: the manual order of a board's tickets (L09-US01).
//!
//! A rank is a string of base-36 digits (`0`–`9`, `a`–`z`) read as the
//! fraction `0.<digits>`, never ending with `0`: two ranks then compare as
//! strings exactly as their fractions do (byte order, `COLLATE "C"`). There
//! is always a rank between two others, so moving a ticket writes only its
//! own rank, without renumbering its neighbours.
//!
//! - [`between`] gives a rank strictly between two ranks (either bound left
//!   open), as short as possible.
//! - [`spread`] gives `n` ranks in order between two bounds, evenly, by
//!   halving: used to rank many tickets at once, and to rebalance a board
//!   whose ranks grew long ([`REBALANCE_LEN`]).

/// The digits, in order.
const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
/// Past this length a rank is worth rebalancing: inserting again and again
/// at the same place adds a digit every five times or so.
pub const REBALANCE_LEN: usize = 48;
/// The longest rank stored (a check of the database).
pub const MAX_LEN: usize = 200;

/// Why no rank could be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankError {
    /// A bound is not a rank: empty, a digit outside `0-9a-z`, or ending with `0`.
    Malformed,
    /// The lower bound is not below the upper one.
    Disordered,
}

/// Whether `rank` is well formed.
pub fn is_valid(rank: &str) -> bool {
    !rank.is_empty()
        && rank.len() <= MAX_LEN
        && rank.bytes().all(|b| DIGITS.contains(&b))
        && !rank.ends_with('0')
}

fn digit(b: u8) -> usize {
    DIGITS.iter().position(|d| *d == b).unwrap_or(0)
}

/// A rank strictly between `low` and `high`; `None` leaves that side open.
pub fn between(low: Option<&str>, high: Option<&str>) -> Result<String, RankError> {
    if low.is_some_and(|l| !is_valid(l)) || high.is_some_and(|h| !is_valid(h)) {
        return Err(RankError::Malformed);
    }
    if let (Some(l), Some(h)) = (low, high)
        && l >= h
    {
        return Err(RankError::Disordered);
    }
    Ok(midpoint(low.unwrap_or("").as_bytes(), high.map(str::as_bytes)))
}

/// The fraction between `a` (`""` for 0) and `b` (`None` for 1), `a < b`,
/// neither ending with `0`.
fn midpoint(a: &[u8], b: Option<&[u8]>) -> String {
    if let Some(b) = b {
        // The common prefix, `a` read with trailing zeros.
        let n = b
            .iter()
            .enumerate()
            .take_while(|(i, d)| a.get(*i).copied().unwrap_or(b'0') == **d)
            .count();
        if n > 0 {
            let rest_a = a.get(n..).unwrap_or(&[]);
            let mut out = String::from_utf8_lossy(&b[..n]).into_owned();
            out.push_str(&midpoint(rest_a, Some(&b[n..])));
            return out;
        }
    }
    let low = a.first().map_or(0, |d| digit(*d));
    let high = b.map_or(DIGITS.len(), |b| digit(b[0]));
    if high - low > 1 {
        return char::from(DIGITS[(low + high).div_ceil(2)]).to_string();
    }
    // Consecutive first digits.
    match b {
        Some(b) if b.len() > 1 => char::from(b[0]).to_string(),
        _ => {
            let mut out = char::from(DIGITS[low]).to_string();
            out.push_str(&midpoint(a.get(1..).unwrap_or(&[]), None));
            out
        }
    }
}

/// `n` ranks in order strictly between `low` and `high`, spaced by halving.
pub fn spread(low: Option<&str>, high: Option<&str>, n: usize) -> Result<Vec<String>, RankError> {
    let mut out = Vec::with_capacity(n);
    fill(low, high, n, &mut out)?;
    Ok(out)
}

fn fill(low: Option<&str>, high: Option<&str>, n: usize, out: &mut Vec<String>) -> Result<(), RankError> {
    if n == 0 {
        return Ok(());
    }
    let middle = between(low, high)?;
    let left = n / 2;
    fill(low, Some(&middle), left, out)?;
    out.push(middle.clone());
    fill(Some(&middle), high, n - left - 1, out)
}

/// Puts `items`, in the order of a board's filter, in the board's order:
/// the ranked ones first by rank, then the others as they came.
pub fn sort_by_rank<'r, T>(items: &mut [T], rank_of: impl Fn(&T) -> Option<&'r str>) {
    items.sort_by(|a, b| match (rank_of(a), rank_of(b)) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranked_tickets_come_first_then_the_filter_order() {
        let mut items = vec![
            (1, None),
            (2, Some("m")),
            (3, None),
            (4, Some("c")),
            (5, Some("x")),
        ];
        sort_by_rank(&mut items, |(_, rank)| *rank);
        let ids: Vec<i32> = items.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![4, 2, 5, 1, 3]);
    }

    fn ordered(ranks: &[String]) -> bool {
        ranks.windows(2).all(|w| w[0] < w[1])
    }

    #[test]
    fn between_two_neighbours_sits_strictly_inside() {
        for (low, high) in [
            (None, None),
            (None, Some("1")),
            (Some("i"), None),
            (Some("a"), Some("b")),
            (Some("a"), Some("a1")),
            (Some("az"), Some("b")),
            (Some("z"), None),
            (Some("zz"), None),
            (None, Some("01")),
            (Some("0001"), Some("0002")),
            (Some("49"), Some("5")),
        ] {
            let rank = between(low, high).unwrap();
            assert!(is_valid(&rank), "{low:?} {high:?} gave {rank}");
            assert!(low.is_none_or(|l| l < rank.as_str()), "{low:?} < {rank}");
            assert!(high.is_none_or(|h| rank.as_str() < h), "{rank} < {high:?}");
        }
    }

    #[test]
    fn the_first_rank_is_the_middle_and_short() {
        assert_eq!(between(None, None).unwrap(), "i");
        assert_eq!(between(Some("a"), Some("c")).unwrap(), "b");
        assert_eq!(between(Some("a"), Some("b")).unwrap(), "ai");
    }

    #[test]
    fn malformed_or_disordered_bounds_are_refused() {
        assert_eq!(between(Some(""), None), Err(RankError::Malformed));
        assert_eq!(between(Some("a0"), None), Err(RankError::Malformed));
        assert_eq!(between(Some("A"), None), Err(RankError::Malformed));
        assert_eq!(between(Some("b"), Some("a")), Err(RankError::Disordered));
        assert_eq!(between(Some("b"), Some("b")), Err(RankError::Disordered));
    }

    #[test]
    fn appending_again_and_again_grows_slowly() {
        let mut last: Option<String> = None;
        let mut ranks = Vec::new();
        for _ in 0..1_000 {
            let next = between(last.as_deref(), None).unwrap();
            ranks.push(next.clone());
            last = Some(next);
        }
        assert!(ordered(&ranks));
        assert!(ranks.last().unwrap().len() <= 200);
    }

    #[test]
    fn inserting_at_the_same_place_needs_a_rebalance_only_after_many_moves() {
        let (low, mut high) = ("a".to_owned(), "b".to_owned());
        let mut moves = 0;
        while high.len() <= REBALANCE_LEN {
            high = between(Some(&low), Some(&high)).unwrap();
            moves += 1;
        }
        assert!(moves > 200, "{moves}");
    }

    #[test]
    fn spread_gives_n_short_ordered_ranks() {
        let ranks = spread(None, None, 1_000).unwrap();
        assert_eq!(ranks.len(), 1_000);
        assert!(ordered(&ranks));
        assert!(
            ranks.iter().all(|r| is_valid(r) && r.len() <= 4),
            "{:?}",
            ranks.iter().map(String::len).max()
        );
        let after = spread(Some("y"), None, 10).unwrap();
        assert!(ordered(&after) && after[0].as_str() > "y");
        let inside = spread(Some("a"), Some("b"), 3).unwrap();
        assert!(ordered(&inside) && inside[0].as_str() > "a" && inside[2].as_str() < "b");
        assert!(spread(None, None, 0).unwrap().is_empty());
    }
}
