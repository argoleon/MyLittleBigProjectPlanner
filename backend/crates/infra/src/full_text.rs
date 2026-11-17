//! Full-text search of tickets (specification §6, L06-US03), behind one door
//! so that a dedicated engine can take over later: the query compiler only
//! calls [`prepare`], [`push_matches`] and [`push_relevance`].
//!
//! PostgreSQL implementation (migration `full_text`):
//!
//! - Every text is indexed with French and with English stemming, without
//!   accents (`issue.search_vector`: the title weighs A, the description B;
//!   `comment.search_vector`). A word matches in either language.
//! - The words of a search must all be found (AND), each one in the ticket's
//!   text or all in one of its comments; members read every comment, the
//!   portal only public ones ([`Audience`]).
//! - A word the tenant never wrote may be a typo: it also matches its
//!   corrections among the words the tenant wrote (`search_word`), picked by
//!   `domain::text_search`. Exact matches rank above corrected ones.
//!
//! Everything is written for a ticket aliased `i` and bound as parameters.

use std::collections::{HashMap, HashSet};

use domain::text_search::{self, Term};
use sqlx::postgres::PgConnection;
use sqlx::{Postgres, QueryBuilder};

/// Where to look for the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The title only (`summary ~`).
    Summary,
    /// The description only (`description ~`).
    Description,
    /// The title, the description and the comments (`text ~`).
    All,
}

/// Who reads the results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Audience {
    /// Members of the tenant: every comment counts.
    #[default]
    Members,
    /// People outside it, through the request portal: public comments only.
    Portal,
}

/// The texts a query searches, each one as its words with their
/// corrections.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Texts(HashMap<String, Vec<Term>>);

impl Texts {
    /// The words of `text`; none when it was not prepared or holds no word.
    pub fn terms(&self, text: &str) -> &[Term] {
        self.0.get(text).map(Vec::as_slice).unwrap_or_default()
    }

    /// Without the database (`search::sql_of`): words split on anything but
    /// letters and digits, in lower case, never corrected.
    pub fn split<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        Self(
            texts
                .into_iter()
                .map(|text| {
                    let terms = text
                        .split(|c: char| !c.is_alphanumeric())
                        .filter(|word| !word.is_empty())
                        .map(|word| Term {
                            word: word.to_lowercase(),
                            corrections: Vec::new(),
                        })
                        .collect();
                    (text.to_owned(), terms)
                })
                .collect(),
        )
    }
}

/// Prepares the searched `texts`: their words as the index reads them, and
/// the corrections of those the tenant never wrote.
pub async fn prepare(conn: &mut PgConnection, texts: &[String]) -> Result<Texts, sqlx::Error> {
    if texts.is_empty() {
        return Ok(Texts::default());
    }
    let rows = sqlx::query!(
        r#"
        SELECT t.text AS "text!", w.lexeme AS "word!"
        FROM unnest($1::text[]) AS t(text), unnest(to_tsvector('mlbpp_simple', t.text)) AS w
        "#,
        texts
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut words: Vec<String> = rows
        .iter()
        .map(|row| row.word.clone())
        .filter(|word| text_search::correctable(word))
        .collect();
    words.sort();
    words.dedup();
    let known: HashSet<String> =
        sqlx::query_scalar!("SELECT word FROM search_word WHERE word = ANY($1)", &words)
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .collect();
    let unknown: Vec<String> = words.into_iter().filter(|word| !known.contains(word)).collect();
    let mut corrections: HashMap<String, Vec<String>> = HashMap::new();
    if !unknown.is_empty() {
        let edits: Vec<i32> = unknown
            .iter()
            .map(|word| i32::try_from(text_search::max_edits(word)).unwrap_or(0))
            .collect();
        // Candidates: a word within k edits is within 2k for Levenshtein
        // (a swap counts two there), and at most k letters longer or shorter.
        let candidates = sqlx::query!(
            r#"
            SELECT t.term AS "term!", w.word AS "word!"
            FROM unnest($1::text[], $2::int[]) AS t(term, k)
            JOIN search_word w
              ON char_length(w.word) BETWEEN char_length(t.term) - t.k AND char_length(t.term) + t.k
             AND levenshtein_less_equal(w.word, t.term, 2 * t.k) <= 2 * t.k
            "#,
            &unknown,
            &edits
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut by_term: HashMap<&str, Vec<&str>> = HashMap::new();
        for row in &candidates {
            by_term.entry(&row.term).or_default().push(&row.word);
        }
        for (term, words) in by_term {
            corrections.insert(term.to_owned(), text_search::corrections(term, words));
        }
    }

    let mut texts: HashMap<String, Vec<Term>> = texts.iter().map(|text| (text.clone(), Vec::new())).collect();
    for row in rows {
        let corrections = corrections.get(&row.word).cloned().unwrap_or_default();
        if let Some(terms) = texts.get_mut(&row.text) {
            terms.push(Term {
                word: row.word,
                corrections,
            });
        }
    }
    Ok(Texts(texts))
}

/// A condition true when the ticket's `scope` holds every word of `terms`
/// (each one, or one of its corrections); false for no word.
pub fn push_matches(qb: &mut QueryBuilder<Postgres>, scope: Scope, terms: &[Term], audience: Audience) {
    if terms.is_empty() {
        qb.push("false");
        return;
    }
    let weight = match scope {
        Scope::Summary => Some('A'),
        Scope::Description => Some('B'),
        Scope::All => None,
    };
    let query = tsquery(terms, weight, true);
    if scope != Scope::All {
        qb.push("i.search_vector @@ ");
        push_tsquery(qb, &query);
        return;
    }
    // The tickets found by their text, then by their comments: each through
    // its index. (`… OR EXISTS (comments)` would read every ticket.)
    qb.push("i.id IN (SELECT m.id FROM issue m WHERE m.search_vector @@ ");
    push_tsquery(qb, &query);
    qb.push(" UNION ALL SELECT c.issue_id FROM comment c WHERE c.deleted_at IS NULL");
    push_audience(qb, audience);
    qb.push(" AND c.search_vector @@ ");
    push_tsquery(qb, &query);
    qb.push(")");
}

/// How well a ticket matches the searched `texts` (as `real`, higher first):
/// its own text, the corrected words counting half, then half its best
/// comment.
pub fn push_relevance(qb: &mut QueryBuilder<Postgres>, texts: &[&[Term]], audience: Audience) {
    let either = |corrected| {
        texts
            .iter()
            .filter(|terms| !terms.is_empty())
            .map(|terms| format!("({})", tsquery(terms, None, corrected)))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    let (exact, all) = (either(false), either(true));
    if exact.is_empty() {
        qb.push("0::real");
        return;
    }
    qb.push("((ts_rank(i.search_vector, ");
    push_tsquery(qb, &exact);
    qb.push(") + 0.5 * ts_rank(i.search_vector, ");
    push_tsquery(qb, &all);
    qb.push(") + 0.5 * COALESCE((SELECT max(ts_rank(c.search_vector, ");
    push_tsquery(qb, &all);
    qb.push(")) FROM comment c WHERE c.issue_id = i.id AND c.deleted_at IS NULL");
    push_audience(qb, audience);
    qb.push("), 0))::real)");
}

fn push_audience(qb: &mut QueryBuilder<Postgres>, audience: Audience) {
    if audience == Audience::Portal {
        qb.push(" AND c.visibility = 'public'");
    }
}

/// `query` read with French and with English stemming: a word matches in
/// either language. In a subquery, so that PostgreSQL builds it once per
/// search: written bare, a prepared statement's generic plan rebuilds it for
/// every row (seven times slower on 20,000 matches).
fn push_tsquery(qb: &mut QueryBuilder<Postgres>, query: &str) {
    qb.push("(SELECT to_tsquery('mlbpp_fr', ")
        .push_bind(query.to_owned())
        .push(") || to_tsquery('mlbpp_en', ")
        .push_bind(query.to_owned())
        .push("))");
}

/// The `tsquery` text of `terms`: all of them, each one as itself or, when
/// `corrected`, one of its corrections; restricted to the title (`A`) or the
/// description (`B`) by `weight`. Words are quoted: nothing in them is read
/// as an operator.
fn tsquery(terms: &[Term], weight: Option<char>, corrected: bool) -> String {
    let quote = |word: &str| {
        let mut quoted = format!("'{}'", word.replace('\\', "\\\\").replace('\'', "''"));
        if let Some(weight) = weight {
            quoted.push(':');
            quoted.push(weight);
        }
        quoted
    };
    terms
        .iter()
        .map(|term| {
            let mut either = vec![quote(&term.word)];
            if corrected {
                either.extend(term.corrections.iter().map(|word| quote(word)));
            }
            if either.len() == 1 {
                either.remove(0)
            } else {
                format!("({})", either.join(" | "))
            }
        })
        .collect::<Vec<_>>()
        .join(" & ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(word: &str, corrections: &[&str]) -> Term {
        Term {
            word: word.to_owned(),
            corrections: corrections.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    #[test]
    fn every_word_is_needed_each_one_or_a_correction() {
        let terms = [term("outgae", &["outage"]), term("directory", &[])];
        assert_eq!(tsquery(&terms, None, true), "('outgae' | 'outage') & 'directory'");
        assert_eq!(tsquery(&terms, None, false), "'outgae' & 'directory'");
        assert_eq!(
            tsquery(&terms, Some('A'), true),
            "('outgae':A | 'outage':A) & 'directory':A"
        );
    }

    #[test]
    fn nothing_in_a_word_reads_as_an_operator() {
        let terms = [term("o'brien", &[]), term("a\\b|c&!d", &[])];
        assert_eq!(tsquery(&terms, None, true), r"'o''brien' & 'a\\b|c&!d'");
    }

    #[test]
    fn words_are_split_without_the_database_for_sql_of() {
        let texts = Texts::split(["Outage, of the DIRECTORY"]);
        let words: Vec<&str> = texts
            .terms("Outage, of the DIRECTORY")
            .iter()
            .map(|t| t.word.as_str())
            .collect();
        assert_eq!(words, ["outage", "of", "the", "directory"]);
        assert!(texts.terms("unknown").is_empty());
    }

    #[test]
    fn a_text_without_words_matches_nothing() {
        let mut qb = QueryBuilder::<Postgres>::new("");
        push_matches(&mut qb, Scope::All, &[], Audience::Members);
        assert_eq!(qb.into_string(), "false");
    }

    #[test]
    fn the_portal_reads_public_comments_only() {
        let terms = [term("outage", &[])];
        let sql = |audience| {
            let mut qb = QueryBuilder::<Postgres>::new("");
            push_matches(&mut qb, Scope::All, &terms, audience);
            qb.into_string()
        };
        assert!(sql(Audience::Portal).contains("c.deleted_at IS NULL AND c.visibility = 'public' AND"));
        assert!(!sql(Audience::Members).contains("visibility"));
        let mut qb = QueryBuilder::<Postgres>::new("");
        push_matches(&mut qb, Scope::Summary, &terms, Audience::Members);
        assert!(!qb.into_string().contains("comment"), "comments only for text ~");
    }
}
