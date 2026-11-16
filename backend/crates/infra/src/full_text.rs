//! Full-text matching of tickets (specification §6), behind one door so
//! that a dedicated engine can take over later.
//!
//! L06-US01 matches case-insensitively on the summary and the text of the
//! description (the `text` members of its Tiptap document). L06-US03 swaps
//! in `tsvector` and `pg_trgm` indexes and adds the comments, without the
//! query language or its compiler changing: they only call [`push_contains`].
//!
//! Everything is written for a ticket aliased `i` and bound as parameters.

use sqlx::{Postgres, QueryBuilder};

/// Where to look for the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The summary only.
    Summary,
    /// The description only.
    Description,
    /// Everything that reads as the ticket's text (`text ~ "…"`).
    All,
}

/// The plain text of the description: its text runs, as a JSON array.
const DESCRIPTION_TEXT: &str = "COALESCE(jsonb_path_query_array(i.description, 'lax $.**.text')::text, '')";

/// A condition true when the ticket's `scope` holds `text`, any case.
pub fn push_contains(qb: &mut QueryBuilder<Postgres>, scope: Scope, text: &str) {
    let pattern = crate::invitations::like_pattern(text);
    match scope {
        Scope::Summary => {
            qb.push("i.summary ILIKE ").push_bind(pattern);
        }
        Scope::Description => {
            qb.push(DESCRIPTION_TEXT).push(" ILIKE ").push_bind(pattern);
        }
        Scope::All => {
            qb.push("(i.summary ILIKE ")
                .push_bind(pattern.clone())
                .push(" OR ")
                .push(DESCRIPTION_TEXT)
                .push(" ILIKE ")
                .push_bind(pattern)
                .push(")");
        }
    }
}
