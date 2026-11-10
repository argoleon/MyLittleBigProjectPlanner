//! Keyset pagination on `(created_at, id)`.
//!
//! Lists are ordered by creation time, then id (UUID v7, so also time
//! ordered). A page starts strictly after the last row of the previous page:
//!
//! ```sql
//! WHERE ($1::timestamptz IS NULL OR (created_at, id) > ($1, $2))
//! ORDER BY created_at, id
//! LIMIT $3   -- page size + 1, to know whether a next page exists
//! ```
//!
//! Unlike `OFFSET`, this stays fast on large tables (with an index on
//! `(tenant_id, created_at, id)`) and never skips or repeats a row when
//! rows are inserted between two pages.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Position after which the next page starts: the last row already seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keyset {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

/// One page of rows and, when more rows follow, the position of the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<Keyset>,
}

/// Builds a page from rows fetched with `LIMIT limit + 1`: the extra row only
/// tells that a next page exists and is dropped.
pub fn page_from_rows<T>(mut rows: Vec<T>, limit: usize, keyset: impl Fn(&T) -> Keyset) -> Page<T> {
    if rows.len() > limit {
        rows.truncate(limit);
        let next = rows.last().map(keyset);
        Page { items: rows, next }
    } else {
        Page {
            items: rows,
            next: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u128) -> Keyset {
        Keyset {
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            id: Uuid::from_u128(n),
        }
    }

    #[test]
    fn extra_row_means_a_next_page_after_the_last_kept_row() {
        let page = page_from_rows(vec![1_u128, 2, 3], 2, |n| key(*n));
        assert_eq!(page.items, vec![1, 2]);
        assert_eq!(page.next, Some(key(2)));
    }

    #[test]
    fn no_extra_row_means_last_page() {
        assert_eq!(page_from_rows(vec![1_u128, 2], 2, |n| key(*n)).next, None);
        assert_eq!(page_from_rows(Vec::<u128>::new(), 2, |n| key(*n)).next, None);
    }
}
